#!/usr/bin/env bash
set -euo pipefail

# Prepare a release commit: bump the version everywhere it lives, start the
# CHANGELOG.md section, and check that everything agrees. Commits, tags and
# pushes nothing — `just release <version>` prints those commands.
#
#   Cargo.toml (+ Cargo.lock)   [workspace.package] version → every crate,
#                               `tond --version`, tond-server's current_version
#   dashboard/package.json      "version"
#   docs/content/guides/ci.md   the TOND_VERSION example
#   CHANGELOG.md                "## <version> — <date>" with a draft from the
#                               conventional commits since the last tag, unless
#                               the section already exists (edit it by hand)
#
# Version scheme: SemVer, tags v<version>. 0.x while on the launch track (a
# minor bump may break things), 1.0.0 once the launch gate passes. A version
# with a pre-release part (0.3.0-rc.1) is published as a GitHub pre-release,
# which installs and update checks skip.
#
# Usage: scripts/release-prep.sh <version>   (just release-prep <version>)

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-}"
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || {
    echo "Usage: just release-prep <version>   (SemVer, e.g. 0.2.0 or 1.0.0-rc.1)" >&2
    exit 2
}
cd "$ROOT"

# In-place edit that works with GNU and BSD sed.
sed_i() { local f="$1"; shift; sed "$@" "$f" > "${f}.tmp" && mv "${f}.tmp" "$f"; }

sed_i Cargo.toml -e '/^\[workspace.package\]/,/^\[/s/^version *= *".*"/version = "'"$VERSION"'"/'
cargo update --workspace --offline --quiet
sed_i dashboard/package.json -e 's/^\([[:space:]]*"version": *\)"[^"]*"/\1"'"$VERSION"'"/'
sed_i docs/content/guides/ci.md -e 's/TOND_VERSION\([:=] *\)v[0-9][0-9A-Za-z.+-]*/TOND_VERSION\1v'"$VERSION"'/'
echo "Version set to ${VERSION} (Cargo.toml, Cargo.lock, dashboard/package.json, docs/content/guides/ci.md)"

if grep -q "^## ${VERSION//./\\.}\( \|$\)" CHANGELOG.md 2> /dev/null; then
    echo "CHANGELOG.md already has a ${VERSION} section; review it."
else
    last_tag="$(git describe --tags --abbrev=0 2> /dev/null || true)"
    range="${last_tag:+${last_tag}..}HEAD"
    draft="$(mktemp)"
    {
        echo "## ${VERSION} — $(date +%Y-%m-%d)"
        echo
        echo "<!-- Draft from the commits in ${range}: rewrite it for people running Tond. -->"
        for group in "feat:Features" "fix:Fixes" "perf:Performance"; do
            prefix="${group%%:*}"
            lines="$(git log --no-merges --format='%s' "$range" | grep -E "^${prefix}(\([^)]*\))?!?: " | sed -E 's/^[a-z]+(\([^)]*\))?!?: /- /' || true)"
            [[ -n "$lines" ]] || continue
            echo
            echo "### ${group#*:}"
            echo
            echo "$lines"
        done
        echo
    } > "$draft"
    if [[ -f CHANGELOG.md ]]; then
        # After the header (everything before the first "## " section).
        awk -v draft="$draft" '
            !done && /^## / { while ((getline line < draft) > 0) print line; done = 1 }
            { print }
            END { if (!done) while ((getline line < draft) > 0) print line }
        ' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
    else
        { printf '# Changelog\n\nAll notable changes to Tond. Versions follow SemVer; 0.x releases may break things between minor versions.\n\n'; cat "$draft"; } > CHANGELOG.md
    fi
    rm -f "$draft"
    echo "Started the ${VERSION} section of CHANGELOG.md from ${range}: edit it before tagging."
fi

scripts/check-version.sh "$VERSION"
