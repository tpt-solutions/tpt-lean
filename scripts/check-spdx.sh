#!/usr/bin/env bash
# Verify that every Rust source file in the workspace carries the project SPDX header
# on its first line. Use --fix to add the header where it is missing.
set -euo pipefail
HEADER='// SPDX-License-Identifier: MIT OR Apache-2.0'
fix=0
[[ "${1:-}" == "--fix" ]] && fix=1
root="$(cd "$(dirname "$0")/.." && pwd)"
missing=0
while IFS= read -r -d '' file; do
  if [[ "$(head -n1 "$file" | tr -d '\r')" != "$HEADER" ]]; then
    if [[ $fix -eq 1 ]]; then
      tmp="$(mktemp)"
      { printf '%s\n' "$HEADER"; cat "$file"; } > "$tmp" && mv "$tmp" "$file"
      echo "fixed: ${file#$root/}"
    else
      echo "missing SPDX header: ${file#$root/}"
      missing=$((missing + 1))
    fi
  fi
done < <(find "$root/crates" -name '*.rs' -not -path '*/target/*' -print0)
if [[ $missing -gt 0 ]]; then
  echo "$missing file(s) missing the header"
  exit 1
fi
echo "SPDX headers OK"
