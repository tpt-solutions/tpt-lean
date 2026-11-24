# SPDX-License-Identifier: MIT OR Apache-2.0
"""ctypes tests for the tpt-lean C ABI.

Run after `cargo build -p tpt-lean-ffi`:

    python -m unittest discover -s crates/tpt-lean-ffi/tests/ffi -v

The library path defaults to target/debug/<platform name>; set TPT_LEAN_FFI_LIB to override.
"""

import ctypes
import json
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
HEADER = REPO / "crates" / "tpt-lean-ffi" / "include" / "tpt_lean.h"

OK = 0
INVALID_ARGUMENT = 1
IO = 2


def library_path() -> Path:
    override = os.environ.get("TPT_LEAN_FFI_LIB")
    if override:
        return Path(override)
    names = {"win32": "tpt_lean_ffi.dll", "darwin": "libtpt_lean_ffi.dylib"}
    return REPO / "target" / "debug" / names.get(sys.platform, "libtpt_lean_ffi.so")


def load_library() -> ctypes.CDLL:
    path = library_path()
    if not path.exists():
        raise unittest.SkipTest(f"{path} not built; run `cargo build -p tpt-lean-ffi`")
    lib = ctypes.CDLL(str(path))
    vp = ctypes.c_void_p
    pp = ctypes.POINTER(ctypes.c_void_p)
    lib.tptl_abi_version.restype = ctypes.c_uint32
    lib.tptl_abi_version.argtypes = []
    lib.tptl_context_new.restype = ctypes.c_int
    lib.tptl_context_new.argtypes = [pp]
    lib.tptl_context_free.restype = None
    lib.tptl_context_free.argtypes = [vp]
    lib.tptl_last_error.restype = ctypes.c_char_p
    lib.tptl_last_error.argtypes = [vp]
    lib.tptl_index_open.restype = ctypes.c_int
    lib.tptl_index_open.argtypes = [vp, ctypes.c_char_p, pp]
    lib.tptl_index_free.restype = None
    lib.tptl_index_free.argtypes = [vp]
    lib.tptl_search_query.restype = ctypes.c_int
    lib.tptl_search_query.argtypes = [vp, ctypes.c_char_p, ctypes.c_uint32, pp]
    lib.tptl_search_results_to_json.restype = ctypes.c_char_p
    lib.tptl_search_results_to_json.argtypes = [vp]
    lib.tptl_search_results_free.restype = None
    lib.tptl_search_results_free.argtypes = [vp]
    lib.tptl_format_text.restype = ctypes.c_int
    lib.tptl_format_text.argtypes = [vp, ctypes.c_char_p, pp]
    lib.tptl_syntax_parse.restype = ctypes.c_int
    lib.tptl_syntax_parse.argtypes = [vp, ctypes.c_char_p, pp]
    lib.tptl_diagnostics_parse.restype = ctypes.c_int
    lib.tptl_diagnostics_parse.argtypes = [vp, ctypes.c_char_p, pp]
    lib.tptl_cache_query.restype = ctypes.c_int
    lib.tptl_cache_query.argtypes = [vp, ctypes.c_char_p, pp]
    lib.tptl_string_free.restype = None
    lib.tptl_string_free.argtypes = [vp]
    return lib


def header_functions() -> list[str]:
    text = HEADER.read_text(encoding="utf-8")
    return sorted(set(re.findall(r"\b(tptl_[a-z_]+)\s*\(", text)))


class FfiTest(unittest.TestCase):
    lib: ctypes.CDLL

    @classmethod
    def setUpClass(cls) -> None:
        cls.lib = load_library()

    def setUp(self) -> None:
        self.ctx = ctypes.c_void_p()
        self.assertEqual(self.lib.tptl_context_new(ctypes.byref(self.ctx)), OK)
        self.assertTrue(self.ctx.value)

    def tearDown(self) -> None:
        self.lib.tptl_context_free(self.ctx)

    def take_string(self, status: int, out: ctypes.c_void_p) -> str:
        """Copy an owned string out of the library and free it."""
        self.assertEqual(status, OK, self.last_error())
        self.assertTrue(out.value)
        text = ctypes.cast(out, ctypes.c_char_p).value.decode("utf-8")
        self.lib.tptl_string_free(out)
        return text

    def last_error(self) -> str:
        return self.lib.tptl_last_error(self.ctx).decode("utf-8", "replace")

    def test_abi_version_is_one(self) -> None:
        self.assertEqual(self.lib.tptl_abi_version(), 1)

    def test_status_codes_match_header(self) -> None:
        text = HEADER.read_text(encoding="utf-8")
        expected = {
            "TPTL_OK": 0,
            "TPTL_ERR_INVALID_ARGUMENT": 1,
            "TPTL_ERR_IO": 2,
            "TPTL_ERR_LEAN_NOT_FOUND": 3,
            "TPTL_ERR_INDEX": 4,
            "TPTL_ERR_INTERNAL": 5,
        }
        for name, value in expected.items():
            self.assertIn(f"{name} = {value}", text)

    def test_every_declared_function_is_exported(self) -> None:
        for name in header_functions():
            with self.subTest(name=name):
                self.assertTrue(hasattr(self.lib, name))

    def test_format_text_round_trip_and_free(self) -> None:
        out = ctypes.c_void_p()
        status = self.lib.tptl_format_text(self.ctx, b"def x := 1   \n", ctypes.byref(out))
        self.assertEqual(self.take_string(status, out), "def x := 1\n")

    def test_format_refuses_uncertain_input_and_leaves_out_null(self) -> None:
        out = ctypes.c_void_p()
        status = self.lib.tptl_format_text(self.ctx, b"theorem t := by\n\tsimp\n", ctypes.byref(out))
        self.assertEqual(status, INVALID_ARGUMENT)
        self.assertFalse(out.value)

    def test_error_text_for_null_input(self) -> None:
        out = ctypes.c_void_p()
        status = self.lib.tptl_format_text(self.ctx, None, ctypes.byref(out))
        self.assertEqual(status, INVALID_ARGUMENT)
        self.assertIn("input is null", self.last_error())
        self.assertFalse(out.value)

    def test_error_text_is_empty_before_any_error(self) -> None:
        self.assertEqual(self.last_error(), "")

    def test_error_text_is_replaced_by_next_error(self) -> None:
        out = ctypes.c_void_p()
        self.lib.tptl_format_text(self.ctx, None, ctypes.byref(out))
        self.lib.tptl_cache_query(self.ctx, b"not-a-digest", ctypes.byref(out))
        self.assertIn("64 lowercase hex", self.last_error())

    def test_null_out_pointer_is_invalid_argument(self) -> None:
        status = self.lib.tptl_syntax_parse(self.ctx, b"def x := 1\n", None)
        self.assertEqual(status, INVALID_ARGUMENT)
        self.assertIn("out_json is null", self.last_error())

    def test_free_functions_accept_null(self) -> None:
        self.lib.tptl_string_free(None)
        self.lib.tptl_index_free(None)
        self.lib.tptl_search_results_free(None)
        self.lib.tptl_context_free(None)
        self.assertIsNone(self.lib.tptl_search_results_to_json(None))

    def test_syntax_parse_reports_imports_and_symbols(self) -> None:
        out = ctypes.c_void_p()
        src = b"import Mathlib.Logic\n\ntheorem foo : True := trivial\n"
        status = self.lib.tptl_syntax_parse(self.ctx, src, ctypes.byref(out))
        doc = json.loads(self.take_string(status, out))
        self.assertEqual(doc["imports"][0]["module"], "Mathlib.Logic")
        self.assertEqual(doc["symbols"][0]["name"], "foo")
        self.assertIsInstance(doc["tokens"], list)
        self.assertIsInstance(doc["uncertain"], list)

    def test_diagnostics_parse(self) -> None:
        out = ctypes.c_void_p()
        text = b"Main.lean:3:4: error: unknown identifier 'x'\n"
        status = self.lib.tptl_diagnostics_parse(self.ctx, text, ctypes.byref(out))
        doc = json.loads(self.take_string(status, out))
        self.assertEqual(doc["diagnostics"][0]["line"], 3)
        self.assertEqual(doc["diagnostics"][0]["severity"], "error")

    def test_cache_query_missing_entry(self) -> None:
        out = ctypes.c_void_p()
        status = self.lib.tptl_cache_query(self.ctx, b"f" * 64, ctypes.byref(out))
        if status == IO:
            # No global cache directory on this platform.
            self.assertFalse(out.value)
            return
        doc = json.loads(self.take_string(status, out))
        self.assertFalse(doc["found"])
        self.assertIsNone(doc["entry"])

    def test_cache_query_rejects_path_like_digest(self) -> None:
        out = ctypes.c_void_p()
        status = self.lib.tptl_cache_query(self.ctx, b"../../secret", ctypes.byref(out))
        self.assertEqual(status, INVALID_ARGUMENT)
        self.assertFalse(out.value)

    def test_empty_project_index_and_search(self) -> None:
        with tempfile.TemporaryDirectory(prefix="tpt-ffi-py-") as tmp:
            Path(tmp, "lakefile.toml").write_text('name = "probe"\n', encoding="utf-8")
            idx = ctypes.c_void_p()
            status = self.lib.tptl_index_open(self.ctx, tmp.encode("utf-8"), ctypes.byref(idx))
            self.assertEqual(status, OK, self.last_error())
            try:
                res = ctypes.c_void_p()
                status = self.lib.tptl_search_query(idx, b"nothing_here", 0, ctypes.byref(res))
                self.assertEqual(status, OK, self.last_error())
                text = self.lib.tptl_search_results_to_json(res).decode("utf-8")
                self.assertEqual(json.loads(text), {"items": []})
                self.lib.tptl_search_results_free(res)
            finally:
                self.lib.tptl_index_free(idx)

    def test_index_open_missing_project_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="tpt-ffi-py-") as tmp:
            missing = Path(tmp, "no-such-project")
            idx = ctypes.c_void_p()
            status = self.lib.tptl_index_open(self.ctx, str(missing).encode("utf-8"), ctypes.byref(idx))
            self.assertNotEqual(status, OK)
            self.assertFalse(idx.value)


if __name__ == "__main__":
    unittest.main()
