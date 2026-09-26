#!/usr/bin/env bash
set -euo pipefail

# Assemble the Tond install bundle from already-built artefacts.
#
# Produces dist/tond-linux-<arch>/ (binaries, dashboard, installer, systemd
# units, VERSION, ARCH), dist/tond-linux-<arch>.tar.gz and its .sha256 — the
# same layout the release workflow publishes and install.sh expects in bundled
# mode. Used by the release workflow, the end-to-end tests, and the local VM /
# Hetzner runs, so every path installs exactly what users install.
#
# Build first (for --arch aarch64 use the aarch64-unknown-linux-gnu target):
#   cargo zigbuild -p tond-server -p tond-proxy -p tond-cli --target x86_64-unknown-linux-gnu --release
#   (cd dashboard && pnpm build)
#
# Usage: release/package.sh [--arch x86_64|aarch64] [--bin-dir <dir>] [--version <version>]
#   --arch      CPU architecture of the binaries (default: x86_64)
#   --bin-dir   directory containing tond-server, tond-proxy, tond
#               (default: target/<arch>-unknown-linux-gnu/release)
#   --version   written to VERSION (default: "dev")

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="x86_64"
BIN_DIR=""
VERSION="dev"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --bin-dir) BIN_DIR="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        -h|--help) sed -n '4,21p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64"; ELF_MACHINE="x86-64" ;;
    aarch64|arm64) ARCH="aarch64"; ELF_MACHINE="aarch64" ;;
    *) echo "Unsupported --arch: $ARCH (expected x86_64 or aarch64)" >&2; exit 1 ;;
esac
BIN_DIR="${BIN_DIR:-${ROOT}/target/${ARCH}-unknown-linux-gnu/release}"
NAME="tond-linux-${ARCH}"
BUNDLE="${ROOT}/dist/${NAME}"

for bin in tond-server tond-proxy tond; do
    [[ -f "${BIN_DIR}/${bin}" ]] || { echo "Missing binary: ${BIN_DIR}/${bin}" >&2; exit 1; }
    # Catch packaging the wrong target's binaries under the wrong name.
    if command -v file > /dev/null; then
        info="$(file -b "${BIN_DIR}/${bin}")"
        if [[ "$info" != *ELF* || "$info" != *"$ELF_MACHINE"* ]]; then
            echo "${BIN_DIR}/${bin} is not a Linux ${ARCH} binary: ${info}" >&2
            exit 1
        fi
    fi
done
[[ -f "${ROOT}/dashboard/build/index.html" ]] || {
    echo "Missing dashboard build: run 'pnpm build' in dashboard/" >&2
    exit 1
}

rm -rf "${BUNDLE}" "${BUNDLE}.tar.gz" "${BUNDLE}.tar.gz.sha256"
mkdir -p "${BUNDLE}/dashboard"

cp "${BIN_DIR}/tond-server" "${BIN_DIR}/tond-proxy" "${BIN_DIR}/tond" "${BUNDLE}/"
cp -r "${ROOT}/dashboard/build/." "${BUNDLE}/dashboard/"
cp "${ROOT}/release/install.sh" "${ROOT}/release/update.sh" \
    "${ROOT}/release/tond-server.service" "${ROOT}/release/tond-proxy.service" \
    "${ROOT}/release/tond-build.slice" "${BUNDLE}/"
chmod +x "${BUNDLE}/install.sh" "${BUNDLE}/update.sh" \
    "${BUNDLE}/tond-server" "${BUNDLE}/tond-proxy" "${BUNDLE}/tond"
# install.sh persists VERSION to /opt/tond/VERSION on the host and refuses a
# bundle whose ARCH does not match the host.
echo "${VERSION}" > "${BUNDLE}/VERSION"
echo "${ARCH}" > "${BUNDLE}/ARCH"

tar -czf "${BUNDLE}.tar.gz" -C "${ROOT}/dist" "${NAME}"
# "<sha256>  <file>" — the format install.sh, update.sh and `tond update` verify.
(
    cd "${ROOT}/dist"
    if command -v sha256sum > /dev/null; then
        sha256sum "${NAME}.tar.gz"
    else
        shasum -a 256 "${NAME}.tar.gz"
    fi
) > "${BUNDLE}.tar.gz.sha256"
echo "Bundle: ${BUNDLE} (${VERSION}, ${ARCH})"
echo "Tarball: ${BUNDLE}.tar.gz (+ .sha256)"
