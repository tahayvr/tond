#!/bin/sh
# tond.io/update - Quick updater for Tond
# This script downloads the update.sh bundled in the latest release and runs it.
# Using the release artefact (rather than main branch) ensures the update logic
# matches the version being installed.

set -e

RELEASE_TARBALL_URL="https://github.com/tahayvr/tond/releases/latest/download/tond-linux-x86_64.tar.gz"

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

# Check architecture
if [ "$(uname -m)" != "x86_64" ]; then
    error "Unsupported architecture: $(uname -m). Tond requires x86_64 (amd64)."
    exit 1
fi

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
    curl -fsSL -o "$TMPDIR/tond.tar.gz" "$RELEASE_TARBALL_URL"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMPDIR/tond.tar.gz" "$RELEASE_TARBALL_URL"
else
    error "Neither curl nor wget found. Please install one of them."
    exit 1
fi

tar -xzf "$TMPDIR/tond.tar.gz" -C "$TMPDIR" tond-linux-x86_64/update.sh

chmod +x "$TMPDIR/tond-linux-x86_64/update.sh"

info "Running update..."
"$TMPDIR/tond-linux-x86_64/update.sh" "$@"
