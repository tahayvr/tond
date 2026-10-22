#!/usr/bin/env bash
# The release signing round trip, locally, with a THROWAWAY key:
#
#   1. generate a throwaway minisign key pair in a temp directory (never the
#      real release key),
#   2. sign the bundles with release/sign.sh exactly as the release workflow
#      does,
#   3. publish them in the release layout on a local file server
#      (python3 -m http.server): <base>/download/<tag>/<asset>, <base>/latest,
#   4. download and verify them with the "release signatures" block of each of
#      install.sh, update.sh, web/install.sh and web/update.sh (pointed at the
#      server with TOND_RELEASE_BASE, trusting the throwaway key), under bash
#      and sh, and check that tampering, a foreign key and a missing signature
#      are refused.
#
# Usage: release/tests/signing-test.sh [--dist <dir>] [--fake] [--tag v9.9.9]
#   --dist  directory with tond-linux-{x86_64,aarch64}.tar.gz (+ .sha256), e.g.
#           from `just package-all` (default: dist)
#   --fake  use small stand-in bundles instead (fast; no build needed)
#
# Needs minisign, openssl 3+ (BLAKE2b and Ed25519; on macOS put Homebrew's
# first in PATH), python3, curl.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "${HERE}/../.." && pwd)"
DIST="${ROOT}/dist"
FAKE=false
TAG="v9.9.9"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dist) DIST="$2"; shift 2 ;;
        --fake) FAKE=true; shift ;;
        --tag) TAG="$2"; shift 2 ;;
        -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
for tool in minisign openssl python3 curl; do
    command -v "$tool" > /dev/null || { echo "${tool} is required" >&2; exit 1; }
done

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "  ok: $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL: $*" >&2; }

WORK="$(mktemp -d)"
SERVER_PID=""
cleanup() {
    if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" 2> /dev/null || true; wait "$SERVER_PID" 2> /dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

sha256_line() { # file — "<sha256>  <name>", like package.sh
    (cd "$(dirname "$1")" && if command -v sha256sum > /dev/null; then sha256sum "$(basename "$1")"; else shasum -a 256 "$(basename "$1")"; fi)
}

# ── Bundles ──
mkdir -p "${WORK}/dist"
for arch in x86_64 aarch64; do
    name="tond-linux-${arch}"
    if [[ "$FAKE" == true ]]; then
        mkdir -p "${WORK}/fake/${name}/dashboard"
        echo "${TAG#v}" > "${WORK}/fake/${name}/VERSION"
        echo "$arch" > "${WORK}/fake/${name}/ARCH"
        printf '#!/bin/sh\necho fake\n' > "${WORK}/fake/${name}/install.sh"
        tar -czf "${WORK}/dist/${name}.tar.gz" -C "${WORK}/fake" "$name"
        sha256_line "${WORK}/dist/${name}.tar.gz" > "${WORK}/dist/${name}.tar.gz.sha256"
    else
        [[ -f "${DIST}/${name}.tar.gz" && -f "${DIST}/${name}.tar.gz.sha256" ]] || {
            echo "No ${DIST}/${name}.tar.gz(.sha256): run 'just profile=release package-all' or use --fake" >&2
            exit 1
        }
        cp "${DIST}/${name}.tar.gz" "${DIST}/${name}.tar.gz.sha256" "${WORK}/dist/"
    fi
done

# ── Throwaway keys and signing (the workflow's step) ──
echo "Signing with a throwaway key (${WORK}/keys)"
mkdir -p "${WORK}/keys"
minisign -G -W -p "${WORK}/keys/test.pub" -s "${WORK}/keys/test.key" > /dev/null
minisign -G -W -p "${WORK}/keys/other.pub" -s "${WORK}/keys/other.key" > /dev/null
TEST_PUBKEY="$(sed -n 2p "${WORK}/keys/test.pub")"
"${ROOT}/release/sign.sh" --key "${WORK}/keys/test.key" --pubkey "${WORK}/keys/test.pub" \
    --comment "tond ${TAG} local-test" --dist "${WORK}/dist" > "${WORK}/sign.log" \
    && ok "release/sign.sh signed and verified $(grep -c '^Signed' "${WORK}/sign.log") files" \
    || { cat "${WORK}/sign.log" >&2; bad "release/sign.sh failed"; exit 1; }
if "${ROOT}/release/sign.sh" --key "${WORK}/keys/other.key" --pubkey "${WORK}/keys/test.pub" \
    --dist "${WORK}/dist" > /dev/null 2>&1; then
    bad "sign.sh must fail when the key doesn't match the public key"
else
    ok "sign.sh refuses a secret key that doesn't match the committed public key"
fi
# That run left signatures by the other key behind: sign again.
"${ROOT}/release/sign.sh" --key "${WORK}/keys/test.key" --pubkey "${WORK}/keys/test.pub" \
    --comment "tond ${TAG} local-test" --dist "${WORK}/dist" > /dev/null

# ── Release layout on a local file server ──
SRV="${WORK}/srv"
mkdir -p "${SRV}/download/${TAG}" "${SRV}/download/v9.9.8" "${SRV}/download/v9.9.7"
cp "${WORK}/dist/"* "${SRV}/download/${TAG}/"
echo "$TAG" > "${SRV}/latest"
# v9.9.8: tampered tarball (checksum file matches the original).
cp "${WORK}/dist/"* "${SRV}/download/v9.9.8/"
printf 'x' >> "${SRV}/download/v9.9.8/tond-linux-x86_64.tar.gz"
# v9.9.7: consistent checksum, signature by a key nobody trusts / none at all.
cp "${WORK}/dist/tond-linux-x86_64.tar.gz" "${WORK}/dist/tond-linux-x86_64.tar.gz.sha256" "${SRV}/download/v9.9.7/"
minisign -S -s "${WORK}/keys/other.key" -m "${SRV}/download/v9.9.7/tond-linux-x86_64.tar.gz" < /dev/null > /dev/null
mkdir -p "${SRV}/download/v9.9.6"
cp "${WORK}/dist/tond-linux-x86_64.tar.gz" "${WORK}/dist/tond-linux-x86_64.tar.gz.sha256" "${SRV}/download/v9.9.6/"

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$SRV" > "${WORK}/http.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:${PORT}/latest" > /dev/null && break; sleep 0.1; done
BASE="http://127.0.0.1:${PORT}"
echo "Serving the release layout at ${BASE}"

# ── Verify with each script's block ──
for script in install.sh update.sh web/install.sh web/update.sh; do
    block="${WORK}/block-$(echo "$script" | tr / -)"
    sed -n '/^# ── Release signatures (keep in sync/,/^# ── end release signatures ──/p' \
        "${ROOT}/release/${script}" > "$block"
    grep -q '^fetch_verified_release()' "$block" || { bad "${script}: no release block"; continue; }
    for shell in bash sh; do
        out="${WORK}/out"
        run() { # snippet — run it with the block loaded, trusting the throwaway key
            env -u TOND_ALLOW_UNSIGNED TOND_RELEASE_BASE="$BASE" TOND_INSTALL_DIR="${WORK}/no-install" \
                "$shell" -c ". '$block'; TOND_RELEASE_PUBKEY='${TEST_PUBKEY}'; $1" > "$out" 2>&1
        }
        label="${script} (${shell})"
        if run 'test "$(latest_release_tag)" = "'"$TAG"'"'; then ok "${label}: latest is ${TAG}"; else bad "${label}: latest_release_tag: $(cat "$out")"; fi
        for arch in x86_64 aarch64; do
            d="${WORK}/get-${arch}"
            rm -rf "$d" && mkdir -p "$d"
            if run "fetch_verified_release ${TAG} tond-linux-${arch} '$d'" \
                && grep -q "Signature of ${TAG} verified" "$out" \
                && tar -xzf "${d}/tond.tar.gz" -C "$d" \
                && [[ "$(tr -d 'v[:space:]' < "${d}/tond-linux-${arch}/VERSION")" != "" ]] \
                && [[ "$(cat "${d}/tond-linux-${arch}/ARCH")" == "$arch" ]]; then
                ok "${label}: tond-linux-${arch} downloaded, checksum and signature verified (VERSION $(cat "${d}/tond-linux-${arch}/VERSION"))"
            else
                bad "${label}: tond-linux-${arch}: $(cat "$out")"
            fi
        done
        d="${WORK}/neg" && rm -rf "$d" && mkdir -p "$d"
        if run "fetch_verified_release v9.9.8 tond-linux-x86_64 '$d'"; then bad "${label}: a tampered tarball was accepted"; else
            grep -q "checksum" "$out" && ok "${label}: tampered tarball refused" || bad "${label}: tampered: $(cat "$out")"; fi
        if run "fetch_verified_release v9.9.7 tond-linux-x86_64 '$d'"; then bad "${label}: a foreign signature was accepted"; else
            grep -q "does not verify" "$out" && ok "${label}: signature by another key refused" || bad "${label}: foreign key: $(cat "$out")"; fi
        if run "fetch_verified_release v9.9.6 tond-linux-x86_64 '$d'"; then bad "${label}: an unsigned release was accepted"; else
            grep -q "has no signature" "$out" && ok "${label}: unsigned release refused (fail closed)" || bad "${label}: unsigned: $(cat "$out")"; fi
        if run "TOND_ALLOW_UNSIGNED=1 fetch_verified_release v9.9.6 tond-linux-x86_64 '$d'"; then ok "${label}: TOND_ALLOW_UNSIGNED=1 lets an unsigned release through"; else bad "${label}: TOND_ALLOW_UNSIGNED: $(cat "$out")"; fi
        # The key embedded in the script (the real release key) doesn't trust the throwaway signature.
        if env TOND_RELEASE_BASE="$BASE" "$shell" -c ". '$block'; fetch_verified_release ${TAG} tond-linux-x86_64 '$d'" > "$out" 2>&1; then
            bad "${label}: the embedded release key accepted a throwaway signature"
        else
            ok "${label}: the embedded release key refuses the throwaway signature"
        fi
        if run "fetch_verified_release '../${TAG}' tond-linux-x86_64 '$d'"; then bad "${label}: a malformed tag was used"; else ok "${label}: malformed tag refused"; fi
    done
done

echo
echo "signing-test: ${PASS} passed, ${FAIL} failed"
((FAIL == 0))
