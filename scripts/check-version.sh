#!/usr/bin/env bash
set -euo pipefail

# Check that every place that carries Tond's version agrees.
#
#   Cargo.toml          [workspace.package] version (all crates; `tond --version`,
#                       tond-server's current_version, the update check)
#   dashboard/package.json  "version"
#   CHANGELOG.md        has a "## <version>" section (the release notes)
#
# With an argument (a version or a tag, e.g. 0.2.0 or v0.2.0) it must match too;
# the release workflow passes the pushed tag. Versions are SemVer; tags are
# v<version>; the bundle's VERSION file holds <version> without the v.
#
# Usage: scripts/check-version.sh [<version|tag>]

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cargo_version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' "${ROOT}/Cargo.toml" | head -n 1)"
dashboard_version="$(sed -n 's/^[[:space:]]*"version": *"\(.*\)",*$/\1/p' "${ROOT}/dashboard/package.json" | head -n 1)"
errors=0

[[ "$cargo_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || {
    echo "Cargo.toml: '${cargo_version}' is not a SemVer version" >&2
    errors=1
}
if [[ "$dashboard_version" != "$cargo_version" ]]; then
    echo "dashboard/package.json is ${dashboard_version}, Cargo.toml is ${cargo_version}" >&2
    errors=1
fi
if [[ $# -ge 1 ]]; then
    want="${1#refs/tags/}"
    want="${want#v}"
    if [[ "$want" != "$cargo_version" ]]; then
        echo "The tag says ${want}, Cargo.toml says ${cargo_version}: run 'just release-prep ${want}' and tag that commit" >&2
        errors=1
    fi
fi
if ! grep -q "^## ${cargo_version//./\\.}\( \|$\)" "${ROOT}/CHANGELOG.md" 2> /dev/null; then
    echo "CHANGELOG.md has no '## ${cargo_version}' section" >&2
    errors=1
fi
((errors == 0)) || exit 1
echo "Version ${cargo_version}: Cargo.toml, dashboard/package.json and CHANGELOG.md agree"
