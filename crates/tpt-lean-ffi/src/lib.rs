// SPDX-License-Identifier: MIT OR Apache-2.0
//! Stable C ABI for tpt-lean. The public contract is `include/tpt_lean.h`; keep the two in sync
//! (`tests/header.rs` checks that every export is declared and every declaration is exported).
//!
//! Rules: opaque handles, plain C types, explicit lengths and ownership, stable status codes,
//! no panics across the boundary (every export runs under `catch_unwind`).
//!
//! Thread safety: `TptlContext` is thread-safe. `TptlIndex` serializes its calls internally
//! (SQLite connection behind a mutex), so concurrent reads are safe but not parallel.
//! `TptlSearchResults` is immutable after creation. `tptl_last_error` returns a pointer valid
//! until the next call on the same context.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use serde_json::{json, Value};
use tpt_lean_adapter::parse_lean_output;
use tpt_lean_core::dirs::{global_cache_dir, ProjectDirs};
use tpt_lean_core::project;
use tpt_lean_core::{Error, ErrorCode};
use tpt_lean_index::Index;
use tpt_lean_search::SearchOptions;

/// ABI version, separate from the crate version. Breaking ABI changes bump this.
pub const ABI_VERSION: u32 = 1;

/// Largest result count accepted by `tptl_search_query`; larger values are clamped.
pub const MAX_SEARCH_LIMIT: u32 = 1000;

/// Status codes. Values are part of the ABI and never change.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TptlStatus {
    Ok = 0,
    InvalidArgument = 1,
    Io = 2,
    LeanNotFound = 3,
    Index = 4,
    Internal = 5,
}

impl From<&Error> for TptlStatus {
    fn from(e: &Error) -> Self {
        match e.code() {
            ErrorCode::InvalidArgument | ErrorCode::PathOutsideWorkspace => TptlStatus::InvalidArgument,
            ErrorCode::Io | ErrorCode::ProjectNotFound => TptlStatus::Io,
            ErrorCode::LeanNotFound => TptlStatus::LeanNotFound,
            ErrorCode::IndexError => TptlStatus::Index,
            _ => TptlStatus::Internal,
        }
    }
}

/// Opaque context. Holds the last error message for `tptl_last_error`.
pub struct TptlContext {
    last_error: Mutex<CString>,
}

/// Opaque index handle.
pub struct TptlIndex {
    index: Mutex<Index>,
}

/// Opaque search result set. Holds the JSON rendering so the pointer stays valid.
pub struct TptlSearchResults {
    json: CString,
}

/// Lock that ignores poisoning: a panic inside a guarded call is already reported as
/// `TPTL_ERR_INTERNAL`, and the SQLite connection stays usable.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_error(ctx: *mut TptlContext, message: &str) {
    // SAFETY: `ctx` is null or a context returned by `tptl_context_new` and not yet freed,
    // as the caller contract requires. Null is ignored.
    if let Some(ctx) = unsafe { ctx.as_ref() } {
        let text = message.replace('\0', " ");
        *lock(&ctx.last_error) = CString::new(text).unwrap_or_default();
    }
}

/// Run `f`, converting errors and panics into a status and error text on the context.
fn guard<F>(ctx: *mut TptlContext, f: F) -> TptlStatus
where
    F: FnOnce() -> Result<(), (TptlStatus, String)>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => TptlStatus::Ok,
        Ok(Err((status, msg))) => {
            set_error(ctx, &msg);
            status
        }
        Err(_) => {
            set_error(ctx, "internal panic");
            TptlStatus::Internal
        }
    }
}

/// Read a NUL-terminated UTF-8 string argument.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string that stays valid for the call.
unsafe fn c_str<'a>(p: *const c_char, what: &str) -> Result<&'a str, (TptlStatus, String)> {
    if p.is_null() {
        return Err((TptlStatus::InvalidArgument, format!("{what} is null")));
    }
    // SAFETY: non-null and NUL-terminated by the caller contract (see above).
    CStr::from_ptr(p)
        .to_str()
        .map_err(|_| (TptlStatus::InvalidArgument, format!("{what} is not valid UTF-8")))
}

/// Store an owned string in `*out`. The caller frees it with `tptl_string_free`.
///
/// # Safety
/// `out` must be non-null and writable (checked by the caller).
unsafe fn write_string(out: *mut *mut c_char, text: String) -> Result<(), (TptlStatus, String)> {
    let c = CString::new(text).map_err(|_| (TptlStatus::Internal, "NUL in output".to_string()))?;
    // SAFETY: `out` is non-null and writable per the caller contract.
    *out = c.into_raw();
    Ok(())
}

fn err(e: Error) -> (TptlStatus, String) {
    (TptlStatus::from(&e), e.to_string())
}

/// ABI version of this library.
#[no_mangle]
pub extern "C" fn tptl_abi_version() -> u32 {
    catch_unwind(|| ABI_VERSION).unwrap_or(0)
}

/// Create a context. Free it with `tptl_context_free`.
#[no_mangle]
pub unsafe extern "C" fn tptl_context_new(out_ctx: *mut *mut TptlContext) -> TptlStatus {
    guard(ptr::null_mut(), || {
        if out_ctx.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_ctx is null".into()));
        }
        let ctx = Box::new(TptlContext {
            last_error: Mutex::new(CString::default()),
        });
        // SAFETY: `out_ctx` is non-null and writable per the caller contract.
        *out_ctx = Box::into_raw(ctx);
        Ok(())
    })
}

/// Free a context. Null is accepted. Handles derived from it are not freed by this call.
#[no_mangle]
pub unsafe extern "C" fn tptl_context_free(ctx: *mut TptlContext) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !ctx.is_null() {
            // SAFETY: non-null and, per the caller contract, created by `tptl_context_new`
            // and not yet freed.
            drop(Box::from_raw(ctx));
        }
    }));
}

/// Last error message for this context, or an empty string. Valid until the next call on `ctx`.
#[no_mangle]
pub unsafe extern "C" fn tptl_last_error(ctx: *mut TptlContext) -> *const c_char {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: null is handled; non-null must be a live context per the caller contract.
        match ctx.as_ref() {
            Some(c) => lock(&c.last_error).as_ptr(),
            None => c"".as_ptr(),
        }
    }))
    .unwrap_or(c"".as_ptr())
}

/// Open the index of the Lean project containing `project_path` (a file or directory).
#[no_mangle]
pub unsafe extern "C" fn tptl_index_open(
    ctx: *mut TptlContext,
    project_path: *const c_char,
    out_index: *mut *mut TptlIndex,
) -> TptlStatus {
    guard(ctx, || {
        if out_index.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_index is null".into()));
        }
        // SAFETY: `project_path` is null or a valid C string (checked by `c_str`).
        let path = unsafe { c_str(project_path, "project_path")? };
        let proj = project::discover(Path::new(path)).map_err(err)?;
        let dirs = ProjectDirs::new(&proj.root);
        let index = Index::open(&dirs.index_db()).map_err(err)?;
        let handle = Box::new(TptlIndex {
            index: Mutex::new(index),
        });
        // SAFETY: `out_index` is non-null and writable per the caller contract.
        *out_index = Box::into_raw(handle);
        Ok(())
    })
}

/// Free an index handle. Null is accepted.
#[no_mangle]
pub unsafe extern "C" fn tptl_index_free(index: *mut TptlIndex) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !index.is_null() {
            // SAFETY: non-null and created by `tptl_index_open`, not yet freed (caller contract).
            drop(Box::from_raw(index));
        }
    }));
}

/// Search the index. `limit` of 0 means 10; values above `MAX_SEARCH_LIMIT` are clamped.
#[no_mangle]
pub unsafe extern "C" fn tptl_search_query(
    index: *mut TptlIndex,
    query: *const c_char,
    limit: u32,
    out_results: *mut *mut TptlSearchResults,
) -> TptlStatus {
    guard(ptr::null_mut(), || {
        if out_results.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_results is null".into()));
        }
        // SAFETY: null is handled; non-null must be a live index handle (caller contract).
        let index =
            unsafe { index.as_ref() }.ok_or((TptlStatus::InvalidArgument, "index is null".to_string()))?;
        // SAFETY: `query` is null or a valid C string (checked by `c_str`).
        let q = unsafe { c_str(query, "query")? };
        let effective = match limit {
            0 => 10,
            n => n.min(MAX_SEARCH_LIMIT),
        };
        let opts = SearchOptions {
            limit: effective as usize,
            ..Default::default()
        };
        let guard_index = lock(&index.index);
        let hits = tpt_lean_search::search(&guard_index, q, &opts).map_err(err)?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "name": h.name, "kind": h.kind, "package": h.package,
                    "source": h.file, "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        let json = json!({ "items": items }).to_string();
        let json = CString::new(json).map_err(|_| (TptlStatus::Internal, "NUL in result".to_string()))?;
        // SAFETY: `out_results` is non-null and writable (checked above).
        unsafe { *out_results = Box::into_raw(Box::new(TptlSearchResults { json })) };
        Ok(())
    })
}

/// JSON text of a result set. Valid until `tptl_search_results_free`. Null if `results` is null.
#[no_mangle]
pub unsafe extern "C" fn tptl_search_results_to_json(results: *const TptlSearchResults) -> *const c_char {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: null is handled; non-null must be live (caller contract).
        match unsafe { results.as_ref() } {
            Some(r) => r.json.as_ptr(),
            None => ptr::null(),
        }
    }))
    .unwrap_or(ptr::null())
}

/// Free a result set. Null is accepted.
#[no_mangle]
pub unsafe extern "C" fn tptl_search_results_free(results: *mut TptlSearchResults) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !results.is_null() {
            // SAFETY: non-null and created by `tptl_search_query`, not yet freed (caller contract).
            drop(Box::from_raw(results));
        }
    }));
}

/// Format Lean source. On success `*out_formatted` is an owned string; free it with `tptl_string_free`.
/// Returns `TPTL_ERR_INVALID_ARGUMENT` when the input cannot be formatted safely.
#[no_mangle]
pub unsafe extern "C" fn tptl_format_text(
    ctx: *mut TptlContext,
    input: *const c_char,
    out_formatted: *mut *mut c_char,
) -> TptlStatus {
    guard(ctx, || {
        if out_formatted.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_formatted is null".into()));
        }
        // SAFETY: `input` is null or a valid C string (checked by `c_str`).
        let text = unsafe { c_str(input, "input")? };
        let out =
            tpt_lean_fmt::format_text(text).map_err(|e| (TptlStatus::InvalidArgument, e.to_string()))?;
        // SAFETY: `out_formatted` is non-null and writable (checked above).
        unsafe { write_string(out_formatted, out.output)? };
        Ok(())
    })
}

/// Parse Lean source into a JSON document: `source_len`, `tokens` (kind, start, end byte
/// offsets), `imports`, `symbols`, and `uncertain` regions. Offsets are UTF-8 byte offsets.
/// On success `*out_json` is owned by the caller: free with `tptl_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tptl_syntax_parse(
    ctx: *mut TptlContext,
    source: *const c_char,
    out_json: *mut *mut c_char,
) -> TptlStatus {
    guard(ctx, || {
        if out_json.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_json is null".into()));
        }
        // SAFETY: `source` is null or a valid C string (checked by `c_str`).
        let src = unsafe { c_str(source, "source")? };
        let parsed = tpt_lean_syntax::parse(src);
        let tokens: Vec<Value> = parsed
            .tokens
            .iter()
            .map(|t| json!({ "kind": format!("{:?}", t.kind), "start": t.range.start, "end": t.range.end }))
            .collect();
        let imports: Vec<Value> = parsed
            .imports
            .iter()
            .map(
                |i| json!({ "module": i.module, "line": i.line, "start": i.range.start, "end": i.range.end }),
            )
            .collect();
        let symbols: Vec<Value> = parsed
            .symbols
            .iter()
            .map(|s| {
                json!({
                    "name": s.name, "short_name": s.short_name, "kind": s.kind,
                    "namespace": s.namespace, "line": s.line,
                    "docstring": s.docstring, "is_private": s.is_private,
                    "start": s.range.start, "end": s.range.end,
                })
            })
            .collect();
        let uncertain: Vec<Value> = parsed
            .uncertain
            .iter()
            .map(|u| json!({ "reason": u.reason, "start": u.range.start, "end": u.range.end }))
            .collect();
        let doc = json!({
            "source_len": parsed.source_len,
            "tokens": tokens,
            "imports": imports,
            "symbols": symbols,
            "uncertain": uncertain,
        });
        // SAFETY: `out_json` is non-null and writable (checked above).
        unsafe { write_string(out_json, doc.to_string())? };
        Ok(())
    })
}

/// Parse Lean CLI output (`file:line:col: severity: message` records) into JSON:
/// `{"diagnostics": [{file, line, column, severity, message}]}`. Unrecognised lines are ignored.
/// On success `*out_json` is owned by the caller: free with `tptl_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tptl_diagnostics_parse(
    ctx: *mut TptlContext,
    lean_output: *const c_char,
    out_json: *mut *mut c_char,
) -> TptlStatus {
    guard(ctx, || {
        if out_json.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_json is null".into()));
        }
        // SAFETY: `lean_output` is null or a valid C string (checked by `c_str`).
        let text = unsafe { c_str(lean_output, "lean_output")? };
        let diagnostics = parse_lean_output(text);
        let doc = json!({ "diagnostics": diagnostics });
        // SAFETY: `out_json` is non-null and writable (checked above).
        unsafe { write_string(out_json, doc.to_string())? };
        Ok(())
    })
}

/// Look up a cache entry by its digest (64 lowercase hex characters, as produced by
/// `CacheKey::digest`) in `<global cache dir>/entries/<digest>.json`. Writes JSON:
/// `{"digest", "found", "corrupt", "entry"}`; a missing entry is `found: false`, not an error.
/// The lookup is read-only. Nothing in this library writes entries yet.
/// On success `*out_json` is owned by the caller: free with `tptl_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tptl_cache_query(
    ctx: *mut TptlContext,
    digest: *const c_char,
    out_json: *mut *mut c_char,
) -> TptlStatus {
    guard(ctx, || {
        if out_json.is_null() {
            return Err((TptlStatus::InvalidArgument, "out_json is null".into()));
        }
        // SAFETY: `digest` is null or a valid C string (checked by `c_str`).
        let digest = unsafe { c_str(digest, "digest")? };
        let doc = cache_lookup(digest)?;
        // SAFETY: `out_json` is non-null and writable (checked above).
        unsafe { write_string(out_json, doc.to_string())? };
        Ok(())
    })
}

fn cache_lookup(digest: &str) -> Result<Value, (TptlStatus, String)> {
    let well_formed = digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return Err((
            TptlStatus::InvalidArgument,
            "digest must be 64 lowercase hex characters".into(),
        ));
    }
    let dir = global_cache_dir().ok_or((
        TptlStatus::Io,
        "no global cache directory on this platform".to_string(),
    ))?;
    let path = dir.join("entries").join(format!("{digest}.json"));
    match std::fs::read(&path) {
        Ok(bytes) => {
            let entry = serde_json::from_slice::<Value>(&bytes).ok();
            Ok(json!({
                "digest": digest,
                "found": true,
                "corrupt": entry.is_none(),
                "entry": entry.unwrap_or(Value::Null),
            }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({
            "digest": digest,
            "found": false,
            "corrupt": false,
            "entry": Value::Null,
        })),
        Err(e) => Err((TptlStatus::Io, format!("cannot read cache entry: {e}"))),
    }
}

/// Free a string returned by this library. Null is accepted.
#[no_mangle]
pub unsafe extern "C" fn tptl_string_free(value: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !value.is_null() {
            // SAFETY: non-null and returned by this library, not yet freed (caller contract).
            drop(CString::from_raw(value));
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn abi_version_is_one() {
        assert_eq!(tptl_abi_version(), 1);
    }

    #[test]
    fn context_error_and_null_argument_handling() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            assert_eq!(tptl_context_new(&mut ctx), TptlStatus::Ok);
            let mut out: *mut c_char = ptr::null_mut();
            let st = tptl_format_text(ctx, ptr::null(), &mut out);
            assert_eq!(st, TptlStatus::InvalidArgument);
            let msg = CStr::from_ptr(tptl_last_error(ctx)).to_str().unwrap();
            assert!(msg.contains("input is null"));
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn format_round_trip_and_free() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            assert_eq!(tptl_context_new(&mut ctx), TptlStatus::Ok);
            let input = CString::new("def x := 1   \n").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(tptl_format_text(ctx, input.as_ptr(), &mut out), TptlStatus::Ok);
            assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "def x := 1\n");
            tptl_string_free(out);
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn uncertain_input_is_rejected() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            tptl_context_new(&mut ctx);
            let input = CString::new("theorem t := by\n\tsimp\n").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tptl_format_text(ctx, input.as_ptr(), &mut out),
                TptlStatus::InvalidArgument
            );
            assert!(out.is_null());
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn index_open_reports_missing_project() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            tptl_context_new(&mut ctx);
            let missing = CString::new(
                std::env::temp_dir()
                    .join("tpt-ffi-no-such-dir-xyz")
                    .display()
                    .to_string(),
            )
            .unwrap();
            let mut idx: *mut TptlIndex = ptr::null_mut();
            let st = tptl_index_open(ctx, missing.as_ptr(), &mut idx);
            assert_ne!(st, TptlStatus::Ok);
            assert!(idx.is_null());
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn syntax_parse_reports_symbols_and_imports() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            tptl_context_new(&mut ctx);
            let src = CString::new("import Mathlib.Logic\n\ntheorem foo : True := trivial\n").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(tptl_syntax_parse(ctx, src.as_ptr(), &mut out), TptlStatus::Ok);
            let v: Value = serde_json::from_str(CStr::from_ptr(out).to_str().unwrap()).unwrap();
            assert_eq!(v["imports"][0]["module"], "Mathlib.Logic");
            assert_eq!(v["symbols"][0]["name"], "foo");
            assert!(v["tokens"].is_array());
            assert!(v["uncertain"].is_array());
            tptl_string_free(out);
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn diagnostics_parse_reads_lean_output() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            tptl_context_new(&mut ctx);
            let text = CString::new("Main.lean:3:4: error: unknown identifier 'x'\n").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tptl_diagnostics_parse(ctx, text.as_ptr(), &mut out),
                TptlStatus::Ok
            );
            let v: Value = serde_json::from_str(CStr::from_ptr(out).to_str().unwrap()).unwrap();
            assert_eq!(v["diagnostics"][0]["line"], 3);
            assert_eq!(v["diagnostics"][0]["severity"], "error");
            tptl_string_free(out);
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn cache_query_rejects_malformed_digest() {
        unsafe {
            let mut ctx: *mut TptlContext = ptr::null_mut();
            tptl_context_new(&mut ctx);
            let bad = CString::new("../../etc/passwd").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                tptl_cache_query(ctx, bad.as_ptr(), &mut out),
                TptlStatus::InvalidArgument
            );
            assert!(out.is_null());
            tptl_context_free(ctx);
        }
    }

    #[test]
    fn freeing_null_is_safe() {
        unsafe {
            tptl_context_free(ptr::null_mut());
            tptl_index_free(ptr::null_mut());
            tptl_search_results_free(ptr::null_mut());
            tptl_string_free(ptr::null_mut());
            assert!(tptl_search_results_to_json(ptr::null()).is_null());
        }
    }
}
