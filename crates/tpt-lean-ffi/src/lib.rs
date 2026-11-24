// SPDX-License-Identifier: MIT OR Apache-2.0
//! Stable C ABI for tpt-lean. The public contract is `include/tpt_lean.h`; keep the two in sync.
//!
//! Rules: opaque handles, plain C types, explicit lengths and ownership, stable status codes,
//! no panics across the boundary (every export runs under `catch_unwind`).
//!
//! Thread safety: `TptlContext` is thread-safe. `TptlIndex` serializes its calls internally
//! (SQLite connection behind a mutex). `TptlSearchResults` is immutable after creation.
//! `tptl_last_error` returns a pointer valid until the next call on the same context.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::project;
use tpt_lean_core::{Error, ErrorCode};
use tpt_lean_index::Index;
use tpt_lean_search::SearchOptions;

/// ABI version, separate from the crate version. Breaking ABI changes bump this.
pub const ABI_VERSION: u32 = 1;

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

fn set_error(ctx: *mut TptlContext, message: &str) {
    if let Some(ctx) = unsafe { ctx.as_ref() } {
        let text = message.replace('\0', " ");
        if let Ok(mut slot) = ctx.last_error.lock() {
            *slot = CString::new(text).unwrap_or_default();
        }
    }
}

/// Run `f`, converting panics to `Internal` and storing error text on the context.
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

unsafe fn c_str<'a>(p: *const c_char, what: &str) -> Result<&'a str, (TptlStatus, String)> {
    if p.is_null() {
        return Err((TptlStatus::InvalidArgument, format!("{what} is null")));
    }
    CStr::from_ptr(p)
        .to_str()
        .map_err(|_| (TptlStatus::InvalidArgument, format!("{what} is not valid UTF-8")))
}

fn err(e: Error) -> (TptlStatus, String) {
    (TptlStatus::from(&e), e.to_string())
}

/// ABI version of this library.
#[no_mangle]
pub extern "C" fn tptl_abi_version() -> u32 {
    ABI_VERSION
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
        *out_ctx = Box::into_raw(ctx);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn tptl_context_free(ctx: *mut TptlContext) {
    if !ctx.is_null() {
        drop(Box::from_raw(ctx));
    }
}

/// Last error message for this context, or an empty string. Valid until the next call on `ctx`.
#[no_mangle]
pub unsafe extern "C" fn tptl_last_error(ctx: *mut TptlContext) -> *const c_char {
    match ctx.as_ref() {
        Some(c) => match c.last_error.lock() {
            Ok(slot) => slot.as_ptr(),
            Err(_) => c"".as_ptr(),
        },
        None => c"".as_ptr(),
    }
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
        let path = c_str(project_path, "project_path")?;
        let proj = project::discover(Path::new(path)).map_err(err)?;
        let dirs = ProjectDirs::new(&proj.root);
        let index = Index::open(&dirs.index_db()).map_err(err)?;
        *out_index = Box::into_raw(Box::new(TptlIndex {
            index: Mutex::new(index),
        }));
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn tptl_index_free(index: *mut TptlIndex) {
    if !index.is_null() {
        drop(Box::from_raw(index));
    }
}

/// Search the index. `limit` of 0 means 10.
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
        let index = index
            .as_ref()
            .ok_or((TptlStatus::InvalidArgument, "index is null".to_string()))?;
        let q = c_str(query, "query")?;
        let opts = SearchOptions {
            limit: limit as usize,
            ..Default::default()
        };
        let guard_index = index
            .index
            .lock()
            .map_err(|_| (TptlStatus::Internal, "index lock poisoned".to_string()))?;
        let hits = tpt_lean_search::search(&guard_index, q, &opts).map_err(err)?;
        let items: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "name": h.name, "kind": h.kind, "package": h.package,
                    "source": h.file, "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        let json = serde_json::json!({ "items": items }).to_string();
        let json = CString::new(json).map_err(|_| (TptlStatus::Internal, "NUL in result".to_string()))?;
        *out_results = Box::into_raw(Box::new(TptlSearchResults { json }));
        Ok(())
    })
}

/// JSON text of a result set. Valid until `tptl_search_results_free`.
#[no_mangle]
pub unsafe extern "C" fn tptl_search_results_to_json(results: *const TptlSearchResults) -> *const c_char {
    match results.as_ref() {
        Some(r) => r.json.as_ptr(),
        None => ptr::null(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn tptl_search_results_free(results: *mut TptlSearchResults) {
    if !results.is_null() {
        drop(Box::from_raw(results));
    }
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
        let text = c_str(input, "input")?;
        let out =
            tpt_lean_fmt::format_text(text).map_err(|e| (TptlStatus::InvalidArgument, e.to_string()))?;
        let c = CString::new(out.output).map_err(|_| (TptlStatus::Internal, "NUL in output".to_string()))?;
        *out_formatted = c.into_raw();
        Ok(())
    })
}

/// Free a string returned by this library.
#[no_mangle]
pub unsafe extern "C" fn tptl_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
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
    fn freeing_null_is_safe() {
        unsafe {
            tptl_context_free(ptr::null_mut());
            tptl_index_free(ptr::null_mut());
            tptl_search_results_free(ptr::null_mut());
            tptl_string_free(ptr::null_mut());
        }
    }
}
