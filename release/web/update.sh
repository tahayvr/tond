#!/bin/sh
# tond.io/update - Quick updater for Tond
# Served byte-for-byte at https://tond.io/update (docs/scripts/assemble-site.mjs).
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

# ── Release signatures (keep in sync: install.sh, update.sh, web/install.sh,
# web/update.sh; crates/tond-cli/src/signature.rs has the same policy) ──
#
# Releases are signed with minisign (release/SIGNING.md). A signature that is
# present must verify; a missing one is refused unless the release predates
# signing (TOND_UNSIGNED_RELEASES) or TOND_ALLOW_UNSIGNED=1 is set. The check
# needs only openssl (Ed25519 over the file's BLAKE2b-512 hash, which is what
# minisign's default "ED" signatures are).
TOND_RELEASE_PUBKEY="RWRyw6SNQq+I90HcxHS9ho+7FUE9drT12DfUN0W1vyS7nU3W6PZ0XeXL"
TOND_UNSIGNED_RELEASES="v0.1.0"

# verify_minisig FILE SIGFILE PUBKEY_B64 — succeeds when SIGFILE is a valid
# prehashed minisign signature of FILE by PUBKEY_B64.
verify_minisig() {
    _vm_dir=$(mktemp -d) || return 1
    _vm_rc=1
    if printf '%s' "$3" | base64 -d > "$_vm_dir/pub.bin" 2>/dev/null \
        && sed -n 2p "$2" | base64 -d > "$_vm_dir/sig.bin" 2>/dev/null \
        && [ "$(wc -c < "$_vm_dir/pub.bin")" -eq 42 ] \
        && [ "$(wc -c < "$_vm_dir/sig.bin")" -eq 74 ] \
        && [ "$(head -c 2 "$_vm_dir/pub.bin")" = "Ed" ] \
        && [ "$(head -c 2 "$_vm_dir/sig.bin")" = "ED" ] \
        && [ "$(tail -c +3 "$_vm_dir/pub.bin" | head -c 8 | od -An -tx1)" = "$(tail -c +3 "$_vm_dir/sig.bin" | head -c 8 | od -An -tx1)" ]; then
        # DER SubjectPublicKeyInfo prefix for an Ed25519 key, then the key.
        printf '\060\052\060\005\006\003\053\145\160\003\041\000' > "$_vm_dir/pub.der"
        tail -c 32 "$_vm_dir/pub.bin" >> "$_vm_dir/pub.der"
        tail -c 64 "$_vm_dir/sig.bin" > "$_vm_dir/sig.raw"
        if openssl dgst -blake2b512 -binary "$1" > "$_vm_dir/hash.bin" 2>/dev/null \
            && openssl pkeyutl -verify -pubin -keyform DER -inkey "$_vm_dir/pub.der" \
                -rawin -in "$_vm_dir/hash.bin" -sigfile "$_vm_dir/sig.raw" > /dev/null 2>&1; then
            _vm_rc=0
        fi
    fi
    rm -rf "$_vm_dir"
    return $_vm_rc
}

# check_release_signature TAG TARBALL SIGFILE — apply the policy above.
# SIGFILE may be missing or empty (the release has no signature). Prints why
# and fails when the release must not be used.
check_release_signature() {
    if [ -z "$TOND_RELEASE_PUBKEY" ]; then
        echo "Warning: this installer has no release signing key; only the checksum of $1 was verified." >&2
        return 0
    fi
    if [ -s "$3" ]; then
        if ! command -v openssl > /dev/null 2>&1; then
            echo "openssl is needed to verify the signature of $1. Install it (apt-get install openssl) and try again." >&2
            return 1
        fi
        if verify_minisig "$2" "$3" "$TOND_RELEASE_PUBKEY"; then
            echo "Signature of $1 verified."
            return 0
        fi
        echo "The signature of release $1 does not verify. The download may have been tampered with; nothing was installed." >&2
        return 1
    fi
    case " $TOND_UNSIGNED_RELEASES " in
        *" $1 "*)
            echo "Warning: $1 was published before releases were signed; only its checksum was verified." >&2
            return 0
            ;;
    esac
    if [ "${TOND_ALLOW_UNSIGNED:-}" = "1" ]; then
        echo "Warning: TOND_ALLOW_UNSIGNED=1 is set, so the missing signature of $1 was ignored." >&2
        return 0
    fi
    echo "Release $1 has no signature (.minisig), so it can't be verified and was not installed. If you trust this download anyway, rerun with TOND_ALLOW_UNSIGNED=1." >&2
    return 1
}

# latest_release_tag — the tag of the latest GitHub release (e.g. v0.2.0).
latest_release_tag() {
    _lr_api="https://api.github.com/repos/tahayvr/tond/releases/latest"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$_lr_api"
    else
        wget -qO- "$_lr_api"
    fi | sed -n 's/.*"tag_name": *"\(v[^"]*\)".*/\1/p' | head -n 1
}
# ── end release signatures ──

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

# Check if Tond is installed
if [ ! -d "/opt/tond" ]; then
    error "Tond is not installed."
    error "To install, run: curl -fsSL https://tond.io/install | sudo bash"
    exit 1
fi

# Pin the release first so the tarball, checksum and signature all come from it.
TAG="$(latest_release_tag || true)"
if [ -z "$TAG" ]; then
    error "Could not look up the latest Tond release. Check this server's internet access and try again."
    exit 1
fi
RELEASE_URL="https://github.com/tahayvr/tond/releases/download/${TAG}/${BUNDLE}.tar.gz"
CHECKSUM_URL="${RELEASE_URL}.sha256"

info "Downloading Tond ${TAG}..."

# Create temp directory
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Download the release tarball and extract update.sh from it
if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TMPDIR/tond.tar.gz" "$RELEASE_URL"
    curl -fsSL -o "$TMPDIR/tond.tar.gz.sha256" "$CHECKSUM_URL"
    curl -fsSL -o "$TMPDIR/tond.tar.gz.minisig" "${RELEASE_URL}.minisig" 2>/dev/null || rm -f "$TMPDIR/tond.tar.gz.minisig"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMPDIR/tond.tar.gz" "$RELEASE_URL"
    wget -qO "$TMPDIR/tond.tar.gz.sha256" "$CHECKSUM_URL"
    wget -qO "$TMPDIR/tond.tar.gz.minisig" "${RELEASE_URL}.minisig" 2>/dev/null || rm -f "$TMPDIR/tond.tar.gz.minisig"
else
    error "Neither curl nor wget found. Please install one of them."
    exit 1
fi

info "Verifying checksum..."
if [ "$(awk '{print $1}' "$TMPDIR/tond.tar.gz.sha256")" != "$(hash_file "$TMPDIR/tond.tar.gz")" ]; then
    error "Checksum verification failed. Aborting."
    exit 1
fi

info "Verifying signature..."
if ! check_release_signature "$TAG" "$TMPDIR/tond.tar.gz" "$TMPDIR/tond.tar.gz.minisig"; then
    exit 1
fi

tar -xzf "$TMPDIR/tond.tar.gz" -C "$TMPDIR"
chmod +x "$TMPDIR/${BUNDLE}/update.sh"

# The verified bundle is installed as is (update.sh --bundle): no second
# download. Prefer `sudo tond update` on a working server: it also takes a
# full snapshot first. This is the way back for a server whose tond CLI or
# tond-server is broken.
info "Running update..."
"$TMPDIR/${BUNDLE}/update.sh" --bundle "$TMPDIR/${BUNDLE}" "$@"
