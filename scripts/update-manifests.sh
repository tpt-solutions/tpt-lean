#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Fill the packaging manifests for one release: the version, and the SHA-256 of each archive.
#
#   scripts/update-manifests.sh <version> [SHA256SUMS-file-or-URL]
#
# With no second argument the SHA256SUMS file is downloaded from the GitHub Release for the tag.
# The manifests under packaging/ are rewritten in place from their templates, so run this on a
# clean checkout. Nothing is published: review the diff, then commit it yourself.
#
# Every @SHA256:<target>@ placeholder must find a line in SHA256SUMS, or the script stops before
# writing anything.

set -euo pipefail

usage() {
    sed -n '3,11p' "$0" | sed 's/^# \{0,1\}//'
}

if [ $# -lt 1 ] || [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    usage
    exit $([ $# -lt 1 ] && echo 2 || echo 0)
fi

version="${1#v}"
sums_source="${2:-https://github.com/tpt-solutions/tpt-lean/releases/download/v$version/SHA256SUMS}"

root="$(cd "$(dirname "$0")/.." && pwd)"
sums="$(mktemp)"
trap 'rm -f "$sums"' EXIT

case "$sums_source" in
    http://* | https://*) curl -fsSL -o "$sums" "$sums_source" ;;
    *) cp "$sums_source" "$sums" ;;
esac

# Templates to fill, found by their placeholders.
mapfile -t files < <(grep -rl --include='*.rb' --include='*.json' --include='*.yaml' -e '@VERSION@' "$root/packaging")
[ "${#files[@]}" -gt 0 ] || { echo "no manifest templates under packaging/" >&2; exit 1; }

# Every hash placeholder used by the templates.
mapfile -t targets < <(grep -rhoE '@SHA256:[A-Za-z0-9_.-]+@' "$root/packaging" | sort -u | sed -e 's/^@SHA256://' -e 's/@$//')

# Resolve each target's hash before changing any file.
declare -A hashes=()
for t in "${targets[@]}"; do
    name="$(grep -E "[[:space:]]tpt-lean-$t\.(tar\.gz|zip)\$" "$sums" | head -n 1 | awk '{print $1}')"
    if [ -z "$name" ]; then
        echo "SHA256SUMS has no archive for target $t; nothing was changed" >&2
        exit 1
    fi
    hashes["$t"]="$name"
done

for f in "${files[@]}"; do
    content="$(cat "$f")"
    content="${content//@VERSION@/$version}"
    for t in "${targets[@]}"; do
        content="${content//@SHA256:$t@/${hashes[$t]}}"
    done
    printf '%s\n' "$content" > "$f"
    echo "updated ${f#"$root"/}"
done
