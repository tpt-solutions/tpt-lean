#!/usr/bin/env sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build and run the C harness with gcc or clang (Linux, macOS, MinGW). Requires
# `cargo build -p tpt-lean-ffi` first. NOT VERIFIED in this environment: no gcc or clang was
# available. On Windows with MSVC use build_msvc.bat instead.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
cc="${CC:-cc}"
out="$repo/target/ffi-c"
mkdir -p "$out"
case "$(uname -s)" in
  Darwin) lib="$repo/target/debug/libtpt_lean_ffi.dylib"; rpath="-Wl,-rpath,$repo/target/debug" ;;
  MINGW*|MSYS*|CYGWIN*) lib="$repo/target/debug/tpt_lean_ffi.dll.lib"; rpath="" ;;
  *) lib="$repo/target/debug/libtpt_lean_ffi.so"; rpath="-Wl,-rpath,$repo/target/debug" ;;
esac
if [ ! -f "$lib" ]; then
  echo "missing $lib; run: cargo build -p tpt-lean-ffi" >&2
  exit 1
fi
# shellcheck disable=SC2086
"$cc" -std=c99 -Wall -Wextra -Werror -I"$repo/crates/tpt-lean-ffi/include" \
  "$here/harness.c" "$lib" $rpath -o "$out/harness"
"$out/harness"
