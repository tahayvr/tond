#!/usr/bin/env bash
set -euo pipefail

# Assemble the Tond install bundle from already-built artefacts.
#
# Produces dist/tond-linux-x86_64/ (binaries, dashboard, installer, systemd
# units, VERSION) and dist/tond-linux-x86_64.tar.gz — the same layout the
# release workflow publishes and install.sh expects in bundled mode. Used by
# the release workflow, the end-to-end tests, and the local VM / Hetzner runs,
# so every path installs exactly what users install.
#
# Build first:
#   cargo build -p tond-server -p tond-proxy -p tond-cli --release   (or --profile fast-release)
#   (cd dashboard && pnpm build)
#
# Usage: release/package.sh [--bin-dir <dir>] [--version <version>]
#   --bin-dir   directory containing tond-server, tond-proxy, tond
#               (default: target/release)
#   --version   written to VERSION (default: "dev")

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${ROOT}/target/release"
VERSION="dev"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin-dir) BIN_DIR="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        -h|--help) sed -n '4,20p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

BUNDLE="${ROOT}/dist/tond-linux-x86_64"

for bin in tond-server tond-proxy tond; do
    [[ -f "${BIN_DIR}/${bin}" ]] || { echo "Missing binary: ${BIN_DIR}/${bin}" >&2; exit 1; }
done
[[ -f "${ROOT}/dashboard/build/index.html" ]] || {
    echo "Missing dashboard build: run 'pnpm build' in dashboard/" >&2
    exit 1
}

rm -rf "${BUNDLE}" "${BUNDLE}.tar.gz"
mkdir -p "${BUNDLE}/dashboard"

cp "${BIN_DIR}/tond-server" "${BIN_DIR}/tond-proxy" "${BIN_DIR}/tond" "${BUNDLE}/"
cp -r "${ROOT}/dashboard/build/." "${BUNDLE}/dashboard/"
cp "${ROOT}/release/install.sh" "${ROOT}/release/update.sh" \
    "${ROOT}/release/tond-server.service" "${ROOT}/release/tond-proxy.service" "${BUNDLE}/"
chmod +x "${BUNDLE}/install.sh" "${BUNDLE}/update.sh" \
    "${BUNDLE}/tond-server" "${BUNDLE}/tond-proxy" "${BUNDLE}/tond"
# install.sh persists VERSION to /opt/tond/VERSION on the host.
echo "${VERSION}" > "${BUNDLE}/VERSION"

tar -czf "${BUNDLE}.tar.gz" -C "${ROOT}/dist" tond-linux-x86_64
echo "Bundle: ${BUNDLE} (${VERSION})"
echo "Tarball: ${BUNDLE}.tar.gz"
