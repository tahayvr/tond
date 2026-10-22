#!/bin/sh
# tond.io/install - Quick installer for Tond
# Served byte-for-byte at https://tond.io/install (docs/scripts/assemble-site.mjs).
# It downloads the latest release from GitHub, verifies its checksum and
# signature, and runs that release's own install.sh with your arguments
# (`curl -fsSL https://tond.io/install | sudo bash -s -- <flags>`).

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

# ── Release signatures (keep in sync: install.sh, update.sh, web/install.sh,
# web/update.sh — byte for byte, a test compares them; crates/tond-common/src/release.rs
# and crates/tond-cli/src/signature.rs have the same rules) ──
#
# Where releases come from: TOND_RELEASE_BASE, else the first line of
# <install dir>/RELEASE_BASE (install.sh writes it when run with a
# non-default base), else the public GitHub releases. A release's files are at
# <base>/download/<tag>/tond-linux-<arch>.tar.gz (+ .sha256, .minisig), and
# <base>/latest names the latest release (GitHub redirects it to
# .../releases/tag/<tag>; a plain file server serves a file holding the tag).
#
# Releases are signed with minisign (release/SIGNING.md). A signature that is
# present must verify; a missing one is refused unless the release predates
# signing (TOND_UNSIGNED_RELEASES) or TOND_ALLOW_UNSIGNED=1 is set. The check
# needs only openssl (Ed25519 over the file's BLAKE2b-512 hash, which is what
# minisign's default "ED" signatures are).
TOND_RELEASE_PUBKEY="RWRyw6SNQq+I90HcxHS9ho+7FUE9drT12DfUN0W1vyS7nU3W6PZ0XeXL"
TOND_UNSIGNED_RELEASES="v0.1.0"
TOND_DEFAULT_RELEASE_BASE="https://github.com/tahayvr/tond/releases"

# release_base — the release base URL, without a trailing slash.
release_base() {
    _rb="${TOND_RELEASE_BASE:-}"
    if [ -z "$_rb" ] && [ -r "${TOND_INSTALL_DIR:-/opt/tond}/RELEASE_BASE" ]; then
        _rb="$(head -n 1 "${TOND_INSTALL_DIR:-/opt/tond}/RELEASE_BASE" | tr -d '[:space:]')"
    fi
    _rb="${_rb:-$TOND_DEFAULT_RELEASE_BASE}"
    printf '%s\n' "${_rb%/}"
}

# release_asset_url TAG FILE — where FILE of release TAG is downloaded from.
release_asset_url() {
    printf '%s/download/%s/%s\n' "$(release_base)" "$1" "$2"
}

# valid_release_tag TAG — v<digit>..., only [A-Za-z0-9.+-] (it goes into a URL).
valid_release_tag() {
    case "$1" in v[0-9]*) ;; *) return 1 ;; esac
    case "$1" in *[!A-Za-z0-9.+-]*) return 1 ;; esac
    return 0
}

# release_fetch URL DEST — download URL to DEST with curl or wget.
release_fetch() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    else
        wget -qO "$2" "$1"
    fi
}

# release_sha256 FILE — the file's SHA-256, lowercase hex.
release_sha256() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# latest_release_tag — the tag of the latest release (e.g. v0.2.0).
latest_release_tag() {
    _lr_url="$(release_base)/latest"
    _lr_body=$(mktemp) || return 1
    if command -v curl > /dev/null 2>&1; then
        _lr_final=$(curl -fsSL -o "$_lr_body" -w '%{url_effective}' "$_lr_url") || _lr_final=""
    else
        _lr_final=$(wget -S -O "$_lr_body" "$_lr_url" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r')
    fi
    _lr_tag=$(printf '%s\n' "$_lr_final" | sed -n 's#.*/releases/tag/\([^/?#]*\).*#\1#p')
    valid_release_tag "$_lr_tag" || _lr_tag=$(head -n 1 "$_lr_body" | tr -d '[:space:]')
    rm -f "$_lr_body"
    valid_release_tag "$_lr_tag" || return 1
    printf '%s\n' "$_lr_tag"
}

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

# fetch_verified_release TAG BUNDLE DIR — download BUNDLE.tar.gz of release TAG
# (e.g. v0.2.0 tond-linux-x86_64) to DIR/tond.tar.gz and verify its checksum
# and signature. Prints why and fails when it can't be used.
fetch_verified_release() {
    valid_release_tag "$1" || { echo "Not a release tag: $1" >&2; return 1; }
    _fv_url="$(release_asset_url "$1" "$2.tar.gz")"
    echo "Downloading $2 $1 from $_fv_url"
    release_fetch "$_fv_url" "$3/tond.tar.gz" \
        || { echo "Could not download $_fv_url." >&2; return 1; }
    release_fetch "$_fv_url.sha256" "$3/tond.tar.gz.sha256" \
        || { echo "Could not download the checksum $_fv_url.sha256." >&2; return 1; }
    _fv_expected="$(awk '{print $1}' "$3/tond.tar.gz.sha256" | head -n 1)"
    case "$_fv_expected" in
        *[!0-9a-f]*|"") _fv_expected="malformed" ;;
    esac
    if [ "${#_fv_expected}" -ne 64 ] || [ "$_fv_expected" != "$(release_sha256 "$3/tond.tar.gz")" ]; then
        echo "The checksum of $2 $1 doesn't match: the download may be corrupted or tampered with; nothing was installed." >&2
        return 1
    fi
    echo "Checksum verified."
    # A missing signature (download failure included) is handled by the policy.
    release_fetch "$_fv_url.minisig" "$3/tond.tar.gz.minisig" 2>/dev/null \
        || rm -f "$3/tond.tar.gz.minisig"
    check_release_signature "$1" "$3/tond.tar.gz" "$3/tond.tar.gz.minisig"
}
# ── end release signatures ──

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

# Pin the release first so the tarball, checksum and signature all come from
# it: TOND_VERSION (e.g. 0.2.0), else the latest. TOND_RELEASE_BASE says where
# releases come from (default: GitHub releases).
if [ -n "${TOND_VERSION:-}" ]; then
    TAG="v${TOND_VERSION#v}"
else
    TAG="$(latest_release_tag || true)"
fi
if [ -z "$TAG" ]; then
    error "Could not look up the latest Tond release at $(release_base)/latest. Check this server's internet access and try again."
    exit 1
fi
if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
    error "Neither curl nor wget found. Please install one of them."
    exit 1
fi

info "Downloading Tond ${TAG}..."

# Create temp directory
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

if ! fetch_verified_release "$TAG" "$BUNDLE" "$TMPDIR"; then
    error "Refusing to install an unverified release."
    exit 1
fi

info "Extracting..."
tar -xzf "$TMPDIR/tond.tar.gz" -C "$TMPDIR"

# Find the extracted directory
if [ -d "$TMPDIR/tond" ]; then
    EXTRACTED="$TMPDIR/tond"
elif [ -d "$TMPDIR/${BUNDLE}" ]; then
    EXTRACTED="$TMPDIR/${BUNDLE}"
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
