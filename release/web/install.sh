#!/bin/sh
# tond.io/install - Quick installer for Tond
# This script downloads and installs the latest Tond release from tond.io/install

set -e

RELEASE_URL="https://github.com/tahayvr/tond/releases/latest/download/tond-linux-x86_64.tar.gz"
CHECKSUM_URL="${RELEASE_URL}.sha256"

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
    error "This installer must be run as root."
    error "Try: curl -fsSL https://tond.io/install | sudo bash"
    exit 1
fi

# Check OS
if [ "$(uname -s)" != "Linux" ]; then
    error "Tond only supports Linux servers."
    exit 1
fi

# Check architecture
if [ "$(uname -m)" != "x86_64" ]; then
    error "Unsupported architecture: $(uname -m). Tond requires x86_64 (amd64)."
    exit 1
fi

info "Downloading Tond..."

# Create temp directory
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Download release
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
EXPECTED_HASH=$(awk '{print $1}' "$TMPDIR/tond.tar.gz.sha256")
ACTUAL_HASH=$(hash_file "$TMPDIR/tond.tar.gz")

if [ "$EXPECTED_HASH" != "$ACTUAL_HASH" ]; then
    error "Checksum verification failed. Aborting."
    exit 1
fi

info "Extracting..."
tar -xzf "$TMPDIR/tond.tar.gz" -C "$TMPDIR"

# Find the extracted directory
if [ -d "$TMPDIR/tond" ]; then
    EXTRACTED="$TMPDIR/tond"
elif [ -d "$TMPDIR/tond-linux-x86_64" ]; then
    EXTRACTED="$TMPDIR/tond-linux-x86_64"
else
    error "Could not find extracted files"
    exit 1
fi

# Run installer
if [ -x "$EXTRACTED/install.sh" ]; then
    "$EXTRACTED/install.sh" "$@"
else
    error "Installer not found in downloaded archive"
    exit 1
fi
