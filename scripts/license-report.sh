#!/usr/bin/env bash
# Print a dependency license report (one line per crate and license) for the current workspace.
# Used per release and by the CI "license-report" job. Requires cargo-deny.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "tpt-lean dependency license report"
echo "generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo
cargo deny list
