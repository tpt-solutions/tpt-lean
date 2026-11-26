// SPDX-License-Identifier: MIT OR Apache-2.0

//go:build windows

// Go tests for the tpt-lean C ABI without cgo: the shared library is loaded with syscall.LoadDLL,
// so these tests need no C compiler. Run with:
//
//	cargo build -p tpt-lean-ffi
//	cd crates/tpt-lean-ffi/tests/go && go test ./...
//
// The library path is taken from TPT_LEAN_FFI_DLL, or defaults to the debug build next to the
// workspace. The tests skip when the library is absent.
package tptleanffi

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"unsafe"
)

const (
	statusOK              = 0
	statusInvalidArgument = 1
)

type lib struct {
	dll *syscall.DLL
}

func loadLib(t *testing.T) *lib {
	t.Helper()
	path := os.Getenv("TPT_LEAN_FFI_DLL")
	if path == "" {
		_, file, _, _ := runtime.Caller(0)
		path = filepath.Join(filepath.Dir(file), "..", "..", "..", "..", "target", "debug", "tpt_lean_ffi.dll")
	}
	if _, err := os.Stat(path); err != nil {
		t.Skipf("skipped: %s not built (run cargo build -p tpt-lean-ffi)", path)
	}
	dll, err := syscall.LoadDLL(path)
	if err != nil {
		t.Fatalf("load %s: %v", path, err)
	}
	return &lib{dll: dll}
}

func (l *lib) call(t *testing.T, name string, args ...uintptr) uintptr {
	t.Helper()
	proc, err := l.dll.FindProc(name)
	if err != nil {
		t.Fatalf("missing export %s: %v", name, err)
	}
	r, _, _ := proc.Call(args...)
	return r
}

// cstring returns a NUL-terminated copy of s. The caller keeps the slice alive while the call runs.
func cstring(s string) []byte {
	return append([]byte(s), 0)
}

// goString reads the NUL-terminated C string at p.
func goString(p uintptr) string {
	if p == 0 {
		return ""
	}
	base := unsafe.Pointer(p)
	var out []byte
	for i := 0; ; i++ {
		b := *(*byte)(unsafe.Add(base, i))
		if b == 0 {
			break
		}
		out = append(out, b)
	}
	return string(out)
}

// outPtr is heap memory for a pointer-sized out parameter. Heap objects do not move, so its
// address stays valid while the library writes to it.
func outPtr() *uintptr {
	return new(uintptr)
}

func TestAbiVersionIsOne(t *testing.T) {
	l := loadLib(t)
	if v := l.call(t, "tptl_abi_version"); v != 1 {
		t.Fatalf("abi version %d, want 1", v)
	}
}

func TestContextLifecycleAndNullFree(t *testing.T) {
	l := loadLib(t)
	ctx := outPtr()
	if st := l.call(t, "tptl_context_new", uintptr(unsafe.Pointer(ctx))); st != statusOK || *ctx == 0 {
		t.Fatalf("context_new status %d, ctx %x", st, *ctx)
	}
	l.call(t, "tptl_context_free", *ctx)
	// Freeing a null handle is safe.
	l.call(t, "tptl_context_free", 0)
	l.call(t, "tptl_string_free", 0)
}

func TestFormatTextRemovesTrailingSpaces(t *testing.T) {
	l := loadLib(t)
	ctx := outPtr()
	l.call(t, "tptl_context_new", uintptr(unsafe.Pointer(ctx)))
	defer l.call(t, "tptl_context_free", *ctx)

	input := cstring("def x := 1   \n")
	out := outPtr()
	st := l.call(t, "tptl_format_text", *ctx, uintptr(unsafe.Pointer(&input[0])), uintptr(unsafe.Pointer(out)))
	runtime.KeepAlive(input)
	if st != statusOK {
		t.Fatalf("format_text status %d: %s", st, goString(l.call(t, "tptl_last_error", *ctx)))
	}
	got := goString(*out)
	l.call(t, "tptl_string_free", *out)
	if got != "def x := 1\n" {
		t.Fatalf("formatted %q", got)
	}
}

func TestNullArgumentsReportAnError(t *testing.T) {
	l := loadLib(t)
	ctx := outPtr()
	l.call(t, "tptl_context_new", uintptr(unsafe.Pointer(ctx)))
	defer l.call(t, "tptl_context_free", *ctx)

	out := outPtr()
	st := l.call(t, "tptl_format_text", *ctx, 0, uintptr(unsafe.Pointer(out)))
	if st != statusInvalidArgument {
		t.Fatalf("status %d, want invalid argument", st)
	}
	if msg := goString(l.call(t, "tptl_last_error", *ctx)); msg == "" {
		t.Fatal("no error message after a failed call")
	}
}

func TestSyntaxParseReturnsJSON(t *testing.T) {
	l := loadLib(t)
	ctx := outPtr()
	l.call(t, "tptl_context_new", uintptr(unsafe.Pointer(ctx)))
	defer l.call(t, "tptl_context_free", *ctx)

	src := cstring("import Std\n\ntheorem t : True := trivial\n")
	out := outPtr()
	st := l.call(t, "tptl_syntax_parse", *ctx, uintptr(unsafe.Pointer(&src[0])), uintptr(unsafe.Pointer(out)))
	runtime.KeepAlive(src)
	if st != statusOK {
		t.Fatalf("syntax_parse status %d", st)
	}
	text := goString(*out)
	l.call(t, "tptl_string_free", *out)
	if !strings.Contains(text, "\"Std\"") || !strings.Contains(text, "\"t\"") {
		t.Fatalf("unexpected parse output: %s", text)
	}
}

func TestIndexOpenOfMissingProjectFails(t *testing.T) {
	l := loadLib(t)
	ctx := outPtr()
	l.call(t, "tptl_context_new", uintptr(unsafe.Pointer(ctx)))
	defer l.call(t, "tptl_context_free", *ctx)

	path := cstring(filepath.Join(os.TempDir(), "tpt-lean-no-such-project-for-go-test"))
	index := outPtr()
	st := l.call(t, "tptl_index_open", *ctx, uintptr(unsafe.Pointer(&path[0])), uintptr(unsafe.Pointer(index)))
	runtime.KeepAlive(path)
	if st == statusOK {
		l.call(t, "tptl_index_free", *index)
		t.Fatal("opening a missing project should fail")
	}
}
