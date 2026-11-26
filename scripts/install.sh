#!/usr/bin/env sh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Install tpt-lean and tlean from a GitHub Release.
#
#   sh install.sh [--version VERSION] [--dir DIR] [--dry-run] [--help]
#
#   --version VERSION  a release tag such as v0.1.0 or 0.1.0 (default: the latest release)
#   --dir DIR          install directory (default: $TPT_LEAN_INSTALL_DIR or ~/.local/bin)
#   --dry-run          print what would be downloaded and where it would go; change nothing
#
# The archive is checked against SHA256SUMS from the same release before anything is extracted.
# Nothing is installed when the checksum does not match.

set -eu

REPO="${TPT_LEAN_REPO:-tpt-solutions/tpt-lean}"
BASE="https://github.com/$REPO/releases"

version="latest"
dir="${TPT_LEAN_INSTALL_DIR:-$HOME/.local/bin}"
dry_run=0

usage() {
    sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
    echo "install.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version needs a value"
            version="$2"
            shift 2
            ;;
        --version=*)
            version="${1#--version=}"
            shift
            ;;
        --dir)
            [ $# -ge 2 ] || die "--dir needs a value"
            dir="$2"
            shift 2
            ;;
        --dir=*)
            dir="${1#--dir=}"
            shift
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1 (see --help)"
            ;;
    esac
done

# Map this machine to a release target, or stop.
os_name="$(uname -s)"
arch_name="$(uname -m)"
case "$os_name" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) die "unsupported operating system: $os_name (Windows: use scripts/install.ps1)" ;;
esac
case "$arch_name" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) die "unsupported architecture: $arch_name" ;;
esac
target="$arch_part-$os_part"
archive="tpt-lean-$target.tar.gz"

case "$version" in
    latest) release_url="$BASE/latest/download" ;;
    *) release_url="$BASE/download/v${version#v}" ;;
esac

echo "target:   $target"
echo "archive:  $archive"
echo "source:   $release_url/$archive"
echo "install:  $dir"

if [ "$dry_run" -eq 1 ]; then
    echo "dry run: nothing was downloaded or installed"
    exit 0
fi

command -v curl >/dev/null 2>&1 || die "curl is required"
if command -v sha256sum >/dev/null 2>&1; then
    sha_check() { sha256sum -c --ignore-missing --quiet "$1" >/dev/null 2>&1; }
elif command -v shasum >/dev/null 2>&1; then
    sha_check() { shasum -a 256 -c --ignore-missing "$1" >/dev/null 2>&1; }
else
    die "sha256sum or shasum is required to verify the download"
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tpt-lean-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

curl -fsSL -o "$tmp/$archive" "$release_url/$archive" || die "download failed: $release_url/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$release_url/SHA256SUMS" || die "download failed: SHA256SUMS"

if ! (cd "$tmp" && sha_check SHA256SUMS); then
    die "checksum mismatch for $archive; nothing was installed"
fi
echo "checksum: ok"

mkdir -p "$tmp/extract"
tar -xzf "$tmp/$archive" -C "$tmp/extract"
mkdir -p "$dir"
for bin in tpt-lean tlean; do
    [ -f "$tmp/extract/$bin" ] || die "archive does not contain $bin"
    install -m 0755 "$tmp/extract/$bin" "$dir/$bin"
done
echo "installed tpt-lean and tlean to $dir"

case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "add $dir to your PATH, for example: export PATH=\"$dir:\$PATH\"" ;;
esac
