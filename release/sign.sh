#!/usr/bin/env bash
set -euo pipefail

# Sign release bundles with minisign and check the signatures against a public
# key — the signing step of .github/workflows/release.yml, runnable locally
# with a throwaway key (release/tests/signing-test.sh).
#
# For every dist/tond-linux-<arch>.tar.gz: verify its .sha256, then write
# <file>.minisig for the tarball and for the .sha256 file, with the trusted
# comment "<comment> <file name>", and verify each with --pubkey.
#
# Usage: release/sign.sh --key <secret key file> --pubkey <minisign.pub>
#                        [--comment <text>] [--dist <dir>]
#   The secret key must be passwordless (minisign -G -W). In the workflow the
#   key comes from the MINISIGN_SECRET_KEY secret and --pubkey is the committed
#   release/minisign.pub, so a key that doesn't match it fails the release.

KEY=""
PUBKEY=""
COMMENT="tond"
DIST="dist"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --key) KEY="$2"; shift 2 ;;
        --pubkey) PUBKEY="$2"; shift 2 ;;
        --comment) COMMENT="$2"; shift 2 ;;
        --dist) DIST="$2"; shift 2 ;;
        -h|--help) sed -n '4,17p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

[[ -s "$KEY" ]] || { echo "--key: no secret key at '${KEY}'" >&2; exit 2; }
[[ -s "$PUBKEY" ]] || { echo "--pubkey: no public key at '${PUBKEY}'" >&2; exit 2; }
grep -qv '^untrusted comment:' <(grep -v '^$' "$PUBKEY") || {
    echo "${PUBKEY} has no key yet (see release/SIGNING.md)" >&2
    exit 1
}
command -v minisign > /dev/null || { echo "minisign is required" >&2; exit 1; }

shopt -s nullglob
tarballs=("${DIST}"/tond-linux-*.tar.gz)
((${#tarballs[@]} > 0)) || { echo "No ${DIST}/tond-linux-*.tar.gz to sign" >&2; exit 1; }

sha256_check() {
    if command -v sha256sum > /dev/null; then sha256sum -c "$1"; else shasum -a 256 -c "$1"; fi
}

for tarball in "${tarballs[@]}"; do
    (cd "$(dirname "$tarball")" && sha256_check "$(basename "$tarball").sha256")
    for file in "$tarball" "${tarball}.sha256"; do
        rm -f "${file}.minisig"
        minisign -S -s "$KEY" -m "$file" -t "${COMMENT} $(basename "$file")" < /dev/null
        minisign -V -q -p "$PUBKEY" -m "$file"
        echo "Signed and verified $(basename "$file")"
    done
done
