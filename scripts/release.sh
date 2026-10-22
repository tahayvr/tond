#!/usr/bin/env bash
set -euo pipefail

# Print the exact commands that publish a release. Runs nothing that changes
# anything: no commit, no tag, no push. Pushing the tag starts
# .github/workflows/release.yml (build both architectures, sign with the
# release environment's key, attest, publish the GitHub Release).
#
# Usage: scripts/release.sh <version>   (just release <version>)

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-}"
VERSION="${VERSION#v}"
TAG="v${VERSION}"
[[ -n "$VERSION" ]] || { echo "Usage: just release <version>" >&2; exit 2; }
cd "$ROOT"

scripts/check-version.sh "$VERSION" || {
    echo "Run 'just release-prep ${VERSION}' and commit the result first." >&2
    exit 1
}
problems=()
[[ -z "$(git status --porcelain)" ]] || problems+=("the working tree has uncommitted changes")
git rev-parse -q --verify "refs/tags/${TAG}" > /dev/null && problems+=("the tag ${TAG} already exists locally")
branch="$(git rev-parse --abbrev-ref HEAD)"
[[ "$branch" == main ]] || problems+=("HEAD is on '${branch}', not main")
if ((${#problems[@]} > 0)); then
    echo "Not ready to release ${TAG}:" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
fi

commit="$(git rev-parse --short HEAD)"
echo "Ready to release ${TAG} from ${commit} (${branch})."
echo "Check first: just check && just test (and the hosted e2e, dev-docs/hosted-e2e.md)."
echo
echo "Run these yourself to publish:"
echo
echo "  git push origin main"
echo "  git tag -a ${TAG} -m \"Tond ${VERSION}\" ${commit}"
echo "  git push origin ${TAG}"
echo
echo "Then approve the 'release' environment in the Actions run (it holds the"
echo "signing key), and afterwards: scripts/release-smoke.sh --version ${VERSION} ..."
