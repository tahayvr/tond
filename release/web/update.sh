#!/bin/sh
# tond.io/update - Quick updater for Tond
# This script downloads the update.sh bundled in the latest release and runs it.
# Using the release artefact (rather than main branch) ensures the update logic
# matches the version being installed.

set -e

# Colors (if terminal supports it)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    NC=''
fi

info() { printf "${GREEN}[tond]${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}[tond]${NC} %s\n" "$*"; }
error() { printf "${RED}[tond]${NC} %s\n" "$*" >&2; }

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        error "Neither sha256sum nor shasum found. Please install one of them."
        exit 1
    fi
}

# Check if running as root
if [ "$(id -u)" -ne 0 ]; then
    error "This updater must be run as root."
    error "Try: curl -fsSL https://tond.io/update | sudo bash"
    exit 1
fi

# Check OS
if [ "$(uname -s)" != "Linux" ]; then
    error "Tond only supports Linux servers."
    exit 1
fi

# Check architecture (bundles: tond-linux-x86_64, tond-linux-aarch64)
case "$(uname -m)" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *)
        error "Unsupported architecture: $(uname -m). Tond supports x86_64 (amd64) and aarch64 (arm64)."
        exit 1
        ;;
esac
BUNDLE="tond-linux-${ARCH}"
RELEASE_URL="https://github.com/tahayvr/tond/releases/latest/download/${BUNDLE}.tar.gz"
CHECKSUM_URL="${RELEASE_URL}.sha256"

# Check if Tond is installed
if [ ! -d "/opt/tond" ]; then
    error "Tond is not installed."
    error "To install, run: curl -fsSL https://tond.io/install | sudo bash"
    exit 1
fi

info "Downloading latest Tond release..."

# Create temp directory
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Download the release tarball and extract update.sh from it
if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TMPDIR/tond.tar.gz" "$RELEASE_URL"
    curl -fsSL -o "$TMPDIR/tond.tar.gz.sha256" "$CHECKSUM_URL"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMPDIR/tond.tar.gz" "$RELEASE_URL"
    wget -qO "$TMPDIR/tond.tar.gz.sha256" "$CHECKSUM_URL"
else
    error "Neither curl nor wget found. Please install one of them."
    exit 1
fi

info "Verifying checksum..."
if [ "$(awk '{print $1}' "$TMPDIR/tond.tar.gz.sha256")" != "$(hash_file "$TMPDIR/tond.tar.gz")" ]; then
    error "Checksum verification failed. Aborting."
    exit 1
fi

tar -xzf "$TMPDIR/tond.tar.gz" -C "$TMPDIR" "${BUNDLE}/update.sh"

chmod +x "$TMPDIR/${BUNDLE}/update.sh"

info "Running update..."
"$TMPDIR/${BUNDLE}/update.sh" "$@"
