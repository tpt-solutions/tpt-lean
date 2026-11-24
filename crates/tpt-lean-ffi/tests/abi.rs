// SPDX-License-Identifier: MIT OR Apache-2.0
//! ABI-level tests: invalid-argument matrix, concurrent reads on one index handle, and
//! consistency between `include/tpt_lean.h` and the exported symbols.

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread;

use serde_json::Value;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_ffi::*;
use tpt_lean_index::{default_scan_roots, Index};

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tpt-ffi-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_ctx() -> *mut TptlContext {
    let mut ctx: *mut TptlContext = ptr::null_mut();
    assert_eq!(unsafe { tptl_context_new(&mut ctx) }, TptlStatus::Ok);
    assert!(!ctx.is_null());
    ctx
}

fn last_error(ctx: *mut TptlContext) -> String {
    unsafe { CStr::from_ptr(tptl_last_error(ctx)) }
        .to_string_lossy()
        .into_owned()
}

/// Assert that `status` is `InvalidArgument` and the context reports `message`.
fn expect_invalid(ctx: *mut TptlContext, name: &str, status: TptlStatus, message: &str) {
    assert_eq!(status, TptlStatus::InvalidArgument, "{name}");
    let got = last_error(ctx);
    assert!(got.contains(message), "{name}: expected {message:?}, got {got:?}");
}

/// Build a Lean project with one declaration and index it, so the FFI can open it.
fn indexed_project(dir: &Path) {
    std::fs::write(dir.join("lakefile.toml"), "name = \"probe\"\n").unwrap();
    std::fs::write(
        dir.join("Probe.lean"),
        "theorem add_comm_probe : True := trivial\n",
    )
    .unwrap();
    let dirs = ProjectDirs::new(dir);
    let mut index = Index::open(&dirs.index_db()).unwrap();
    index.refresh(&default_scan_roots(dir, false)).unwrap();
}

#[test]
fn null_and_bad_arguments_return_invalid_argument() {
    let ctx = new_ctx();
    let bad_utf8 = CString::new(vec![0xFFu8, 0xFE]).unwrap();
    let upper = CString::new("A".repeat(64)).unwrap();
    let traversal = CString::new("../../../../etc/passwd").unwrap();
    let zeros = CString::new("0".repeat(64)).unwrap();
    let short = CString::new("abc").unwrap();
    let mut out: *mut c_char = ptr::null_mut();
    let mut idx: *mut TptlIndex = ptr::null_mut();

    unsafe {
        let st = tptl_format_text(ctx, ptr::null(), &mut out);
        expect_invalid(ctx, "format null input", st, "input is null");
        let st = tptl_format_text(ctx, c"x".as_ptr(), ptr::null_mut());
        expect_invalid(ctx, "format null out", st, "out_formatted is null");
        let st = tptl_format_text(ctx, bad_utf8.as_ptr(), &mut out);
        expect_invalid(ctx, "format invalid utf8", st, "input is not valid UTF-8");

        let st = tptl_syntax_parse(ctx, ptr::null(), &mut out);
        expect_invalid(ctx, "syntax null source", st, "source is null");
        let st = tptl_syntax_parse(ctx, c"x".as_ptr(), ptr::null_mut());
        expect_invalid(ctx, "syntax null out", st, "out_json is null");
        let st = tptl_syntax_parse(ctx, bad_utf8.as_ptr(), &mut out);
        expect_invalid(ctx, "syntax invalid utf8", st, "source is not valid UTF-8");

        let st = tptl_diagnostics_parse(ctx, ptr::null(), &mut out);
        expect_invalid(ctx, "diagnostics null input", st, "lean_output is null");
        let st = tptl_diagnostics_parse(ctx, c"".as_ptr(), ptr::null_mut());
        expect_invalid(ctx, "diagnostics null out", st, "out_json is null");

        let st = tptl_cache_query(ctx, ptr::null(), &mut out);
        expect_invalid(ctx, "cache null digest", st, "digest is null");
        let st = tptl_cache_query(ctx, short.as_ptr(), &mut out);
        expect_invalid(ctx, "cache short digest", st, "64 lowercase hex");
        let st = tptl_cache_query(ctx, upper.as_ptr(), &mut out);
        expect_invalid(ctx, "cache uppercase digest", st, "64 lowercase hex");
        let st = tptl_cache_query(ctx, traversal.as_ptr(), &mut out);
        expect_invalid(ctx, "cache path traversal", st, "64 lowercase hex");
        let st = tptl_cache_query(ctx, zeros.as_ptr(), ptr::null_mut());
        expect_invalid(ctx, "cache null out", st, "out_json is null");

        let st = tptl_index_open(ctx, c"does-not-matter".as_ptr(), ptr::null_mut());
        expect_invalid(ctx, "index_open null out", st, "out_index is null");
        let st = tptl_index_open(ctx, ptr::null(), &mut idx);
        expect_invalid(ctx, "index_open null path", st, "project_path is null");
    }
    // Failed calls must leave their out pointers untouched.
    assert!(out.is_null());
    assert!(idx.is_null());
    unsafe { tptl_context_free(ctx) };
}

#[test]
fn search_with_null_handles_reports_invalid_argument() {
    unsafe {
        let mut res: *mut TptlSearchResults = ptr::null_mut();
        assert_eq!(
            tptl_search_query(ptr::null_mut(), c"q".as_ptr(), 0, &mut res),
            TptlStatus::InvalidArgument
        );
        assert!(res.is_null());

        let dir = TempDir::new("nullq");
        indexed_project(dir.path());
        let path = CString::new(dir.path().display().to_string()).unwrap();
        let ctx = new_ctx();
        let mut idx: *mut TptlIndex = ptr::null_mut();
        assert_eq!(tptl_index_open(ctx, path.as_ptr(), &mut idx), TptlStatus::Ok);
        assert_eq!(
            tptl_search_query(idx, ptr::null(), 0, &mut res),
            TptlStatus::InvalidArgument
        );
        assert!(res.is_null());
        assert_eq!(
            tptl_search_query(idx, c"q".as_ptr(), 0, ptr::null_mut()),
            TptlStatus::InvalidArgument
        );
        tptl_index_free(idx);
        tptl_context_free(ctx);
    }
}

#[test]
fn freeing_and_reading_null_handles_is_safe() {
    unsafe {
        tptl_context_free(ptr::null_mut());
        tptl_index_free(ptr::null_mut());
        tptl_search_results_free(ptr::null_mut());
        tptl_string_free(ptr::null_mut());
        assert!(tptl_search_results_to_json(ptr::null()).is_null());
        assert_eq!(CStr::from_ptr(tptl_last_error(ptr::null_mut())).to_bytes(), b"");
    }
}

#[test]
fn concurrent_reads_on_one_index_handle() {
    let dir = TempDir::new("concurrent");
    indexed_project(dir.path());
    let path = CString::new(dir.path().display().to_string()).unwrap();
    let ctx = new_ctx();
    let mut idx: *mut TptlIndex = ptr::null_mut();
    unsafe {
        assert_eq!(tptl_index_open(ctx, path.as_ptr(), &mut idx), TptlStatus::Ok);
    }
    // Raw pointers are not Send; pass the address and rebuild the pointer in each thread.
    let handle = idx as usize;
    let workers: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(move || {
                let idx = handle as *mut TptlIndex;
                for _ in 0..25 {
                    let mut res: *mut TptlSearchResults = ptr::null_mut();
                    let st = unsafe { tptl_search_query(idx, c"add_comm_probe".as_ptr(), 5, &mut res) };
                    assert_eq!(st, TptlStatus::Ok);
                    let json = unsafe { CStr::from_ptr(tptl_search_results_to_json(res)) }
                        .to_str()
                        .unwrap()
                        .to_string();
                    let v: Value = serde_json::from_str(&json).unwrap();
                    assert_eq!(v["items"][0]["name"], "add_comm_probe");
                    unsafe { tptl_search_results_free(res) };
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("reader thread panicked");
    }
    unsafe {
        tptl_index_free(idx);
        tptl_context_free(ctx);
    }
}

#[test]
fn search_limit_zero_means_ten_and_large_limits_are_clamped() {
    let dir = TempDir::new("limits");
    indexed_project(dir.path());
    let path = CString::new(dir.path().display().to_string()).unwrap();
    let ctx = new_ctx();
    let mut idx: *mut TptlIndex = ptr::null_mut();
    unsafe {
        assert_eq!(tptl_index_open(ctx, path.as_ptr(), &mut idx), TptlStatus::Ok);
        let mut res: *mut TptlSearchResults = ptr::null_mut();
        // u32::MAX is clamped, so this must not fail or overflow.
        let st = tptl_search_query(idx, c"add_comm_probe".as_ptr(), u32::MAX, &mut res);
        assert_eq!(st, TptlStatus::Ok);
        tptl_search_results_free(res);
        tptl_index_free(idx);
        tptl_context_free(ctx);
    }
}

#[test]
fn cache_query_missing_entry_is_not_an_error() {
    let mut out: *mut c_char = ptr::null_mut();
    let ctx = new_ctx();
    let digest = CString::new("f".repeat(64)).unwrap();
    let st = unsafe { tptl_cache_query(ctx, digest.as_ptr(), &mut out) };
    if st == TptlStatus::Ok {
        let text = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_string();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["found"], false);
        assert!(v["entry"].is_null());
        unsafe { tptl_string_free(out) };
    } else {
        // Only acceptable when the platform has no global cache directory.
        assert_eq!(st, TptlStatus::Io);
    }
    unsafe { tptl_context_free(ctx) };
}

/// Names declared in the header: `tptl_<name>(`.
fn header_names(header: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = header;
    while let Some(pos) = rest.find("tptl_") {
        let after = &rest[pos..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..end];
        if after[end..].starts_with('(') && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        rest = &after[end..];
    }
    names.sort();
    names
}

/// Exported names in the Rust source: `extern "C" fn tptl_<name>`.
fn source_exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for part in source.split("extern \"C\" fn ").skip(1) {
        let name: String = part
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.starts_with("tptl_") {
            names.push(name);
        }
    }
    names.sort();
    names
}

#[test]
fn header_and_exports_match() {
    let header = include_str!("../include/tpt_lean.h");
    let source = include_str!("../src/lib.rs");
    let declared = header_names(header);
    let exported = source_exports(source);
    assert!(!declared.is_empty());
    assert_eq!(declared, exported, "include/tpt_lean.h and src/lib.rs disagree");
}

#[test]
fn status_codes_are_stable() {
    // The numeric values are part of the ABI and must never change.
    assert_eq!(TptlStatus::Ok as u32, 0);
    assert_eq!(TptlStatus::InvalidArgument as u32, 1);
    assert_eq!(TptlStatus::Io as u32, 2);
    assert_eq!(TptlStatus::LeanNotFound as u32, 3);
    assert_eq!(TptlStatus::Index as u32, 4);
    assert_eq!(TptlStatus::Internal as u32, 5);
    let header = include_str!("../include/tpt_lean.h");
    for (name, value) in [
        ("TPTL_OK", 0),
        ("TPTL_ERR_INVALID_ARGUMENT", 1),
        ("TPTL_ERR_INTERNAL", 5),
    ] {
        assert!(header.contains(&format!("{name} = {value}")), "{name}");
    }
}
