#!/usr/bin/env bash
set -euo pipefail

# Tond Installer
# Run as root on a fresh server running Debian 12, Debian 13, Ubuntu 24.04 LTS
# or Ubuntu 26.04 LTS (x86_64 or aarch64); see the support matrix at
# https://tond.io/docs/support-matrix. Other systems are refused unless you pass
# --allow-unsupported-os.
#
# Quick install: curl -fsSL https://tond.io/install | sudo bash
#
# This script can run in two modes:
#   1. Bundled mode: Binaries and dashboard are in the same directory as this script
#   2. Download mode: Script downloads latest release from GitHub

INSTALL_DIR="/opt/tond"
BIN_DIR="${INSTALL_DIR}/bin"
DATA_DIR="${INSTALL_DIR}/data"
DASHBOARD_DIR="${INSTALL_DIR}/dashboard"
SECRET_ENV_FILE="${DATA_DIR}/tond.env"
# Written by tond-server on first start, root-only (0600), never logged:
SETUP_TOKEN_FILE="${DATA_DIR}/setup-token"    # one-time admin setup token (removed after setup)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# `su` without `-` (common on Debian) keeps a PATH without the sbin
# directories that useradd, groupadd and ufw live in.
export PATH="${PATH}:/usr/local/sbin:/usr/sbin:/sbin"
NON_INTERACTIVE=false
# --allow-unsupported-os (or TOND_ALLOW_UNSUPPORTED_OS=1): install on a
# distribution outside the support matrix, with a warning. Recorded in
# UNSUPPORTED_OS_MARKER so updates don't refuse the same system later.
ALLOW_UNSUPPORTED_OS=false
[[ "${TOND_ALLOW_UNSUPPORTED_OS:-}" == "1" ]] && ALLOW_UNSUPPORTED_OS=true
UNSUPPORTED_OS_MARKER="${INSTALL_DIR}/ALLOW_UNSUPPORTED_OS"
DASHBOARD_DOMAIN=""
PREVIEW_DOMAIN=""
APPS_DOMAIN=""
APPS_DOMAIN_SET=false
# Unprivileged user tond-proxy runs as (see tond-proxy.service); its group
# shares tond.db with tond-server.
TERMINAL_USER="tond"
PROXY_USER="tond-proxy"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Logging
info()  { echo -e "${GREEN}[tond]${NC} $*"; }
warn()  { echo -e "${YELLOW}[tond]${NC} $*"; }
error() { echo -e "${RED}[tond]${NC} $*" >&2; }
fatal() { error "$@"; exit 1; }
step()  { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }

is_valid_domain() {
    local domain=$1
    [[ "$domain" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$ ]]
}

# Host CPU architecture in bundle naming (tond-linux-<arch>): x86_64 or aarch64.
# Empty for unsupported architectures.
host_arch() {
    case "$(uname -m)" in
        x86_64|amd64) echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) echo "" ;;
    esac
}

# Early check: reject a preview domain that is the dashboard domain or a
# parent/child of it. The full registrable-domain check (Public Suffix List)
# runs in tond-server when configure_domains saves it, and fails the install.
# Prints the reason and returns 1 when invalid.
check_preview_domain() {
    local preview=$1 dashboard=$2
    if ! is_valid_domain "$preview"; then
        echo "Invalid preview domain: ${preview}"
        return 1
    fi
    [[ -z "$dashboard" ]] && return 0
    if [[ "$preview" == "$dashboard" || "$preview" == *".${dashboard}" || "$dashboard" == *".${preview}" ]]; then
        echo "The preview domain (${preview}) must be a separate domain you own, not the dashboard domain (${dashboard}), a parent or subdomain of it, or another name on the same registrable domain. Previews run pull-request code; keeping them on another site isolates them from the dashboard."
        return 1
    fi
    return 0
}

# Registrable-ish suffix used by the early checks below: the last two labels.
# tond-server does the real Public Suffix List check when the domains are
# saved; this only catches obvious mistakes before anything is installed.
site_of() {
    awk -F. '{ if (NF >= 2) print $(NF-1) "." $NF; else print $0 }' <<<"$1"
}

# Early check: the apps domain must be a different site from the dashboard
# and preview domains. Prints the reason and returns 1 when invalid.
check_apps_domain() {
    local apps=$1 dashboard=$2 preview=$3
    if ! is_valid_domain "$apps"; then
        echo "Invalid apps domain: ${apps}"
        return 1
    fi
    if [[ -n "$dashboard" && "$(site_of "$apps")" == "$(site_of "$dashboard")" ]]; then
        echo "The apps domain (${apps}) must be on a different site from the dashboard (${dashboard}): apps run your code, and an app on the dashboard's site could send same-site requests to it and set its cookies. Use a separate domain, e.g. example-apps.com."
        return 1
    fi
    if [[ -n "$preview" && "$(site_of "$apps")" == "$(site_of "$preview")" ]]; then
        echo "The apps domain (${apps}) must be on a different site from the preview domain (${preview}): previews run pull-request code and must not share a site with production apps."
        return 1
    fi
    return 0
}

# Default apps domain: the server's IP under a public wildcard DNS service on
# a different site from the dashboard. <ip>.nip.io and <ip>.sslip.io both
# resolve to <ip>; they are different registrable domains, so when the
# dashboard uses one the apps use the other (apps.<ip>.sslip.io would be
# same-site with a <ip>.sslip.io dashboard).
suggest_apps_domain() {
    local dashboard=$1 ip dashed
    ip=$(get_server_ip)
    is_public_ipv4 "$ip" || return 0
    dashed="${ip//./-}"
    if [[ "$(site_of "$dashboard")" == "nip.io" ]]; then
        echo "${dashed}.sslip.io"
    else
        echo "${dashed}.nip.io"
    fi
}

choose_apps_domain() {
    [[ "$APPS_DOMAIN_SET" == false ]] || return 0
    local suggestion
    suggestion="$(suggest_apps_domain "$DASHBOARD_DOMAIN")"
    if [[ "$NON_INTERACTIVE" == false && -r /dev/tty ]]; then
        echo "Apps get automatic addresses <app>.<apps-domain>. Use a domain with a wildcard"
        echo "DNS record on a different site from the dashboard (apps run your code)."
        if [[ -n "$suggestion" ]]; then
            echo "No domain yet? Press Enter to use ${suggestion}, which resolves to this server."
            read -r -p "Apps domain [${suggestion}]: " APPS_DOMAIN < /dev/tty || APPS_DOMAIN=""
            APPS_DOMAIN="${APPS_DOMAIN:-$suggestion}"
        else
            read -r -p "Apps domain (optional, Enter to skip): " APPS_DOMAIN < /dev/tty || APPS_DOMAIN=""
        fi
        APPS_DOMAIN="$(echo "$APPS_DOMAIN" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
        APPS_DOMAIN="${APPS_DOMAIN%.}"
    elif [[ -n "$suggestion" ]]; then
        APPS_DOMAIN="$suggestion"
        info "No --apps-domain given: apps are served at <app>.${APPS_DOMAIN} (resolves to this server)"
    fi
}

mark_pass() { printf "%b✓%b %s\n" "$GREEN" "$NC" "$1"; }
mark_warn() { printf "%b!%b %s\n" "$YELLOW" "$NC" "$1"; }
mark_fail() { printf "%b✗%b %s\n" "$RED" "$NC" "$1"; }

# Progress spinner
spinner() {
    local pid=$1
    local delay=0.1
    local spinstr='|/-\\'
    while [ -d /proc/$pid ]; do
        local temp=${spinstr#?}
        printf " [%c]  " "$spinstr"
        local spinstr=$temp${spinstr%"$temp"}
        sleep $delay
        printf "\b\b\b\b\b\b"
    done
    printf "    \b\b\b\b"
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

# ── Supported systems (keep in sync: install.sh, update.sh; documented in
# docs/content/support-matrix.md) ──
#
# Tond supports these distributions, on x86_64 and aarch64, written as
# "<os-release ID>:<VERSION_ID>:<codename>". Anything else is refused unless
# the operator explicitly allows it (install.sh --allow-unsupported-os).
TOND_SUPPORTED_SYSTEMS="debian:12:bookworm debian:13:trixie ubuntu:24.04:noble ubuntu:26.04:resolute"
TOND_SUPPORTED_SYSTEMS_TEXT="Debian 12, Debian 13, Ubuntu 24.04 LTS and Ubuntu 26.04 LTS (x86_64 or aarch64)"
# Docker's apt repository publishes a new distribution release some time
# after it ships; until then, use the previous release's packages.
TOND_DOCKER_CODENAME_FALLBACK="trixie:bookworm resolute:noble"

# os_release_value FILE KEY — one value from an os-release file. The file is
# parsed, never sourced: sourcing would overwrite this script's variables.
os_release_value() {
    [ -r "$1" ] || return 0
    sed -n "s/^$2=//p" "$1" | head -n 1 | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'\$/\1/"
}

# detect_system [OS_RELEASE_FILE] — sets SYSTEM_ID, SYSTEM_VERSION,
# SYSTEM_CODENAME, SYSTEM_NAME, SYSTEM_LIKE, SYSTEM_UBUNTU_CODENAME,
# SYSTEM_DEBIAN_CODENAME and SYSTEM_SUPPORTED (yes|no).
detect_system() {
    _ds_file="${1:-/etc/os-release}"
    [ -r "$_ds_file" ] || _ds_file="/usr/lib/os-release"
    SYSTEM_ID="$(os_release_value "$_ds_file" ID)"
    SYSTEM_VERSION="$(os_release_value "$_ds_file" VERSION_ID)"
    SYSTEM_CODENAME="$(os_release_value "$_ds_file" VERSION_CODENAME)"
    SYSTEM_NAME="$(os_release_value "$_ds_file" PRETTY_NAME)"
    SYSTEM_LIKE="$(os_release_value "$_ds_file" ID_LIKE)"
    SYSTEM_UBUNTU_CODENAME="$(os_release_value "$_ds_file" UBUNTU_CODENAME)"
    SYSTEM_DEBIAN_CODENAME="$(os_release_value "$_ds_file" DEBIAN_CODENAME)"
    [ -n "$SYSTEM_NAME" ] || SYSTEM_NAME="${SYSTEM_ID:-an unknown system} ${SYSTEM_VERSION}"
    SYSTEM_SUPPORTED=no
    for _ds_entry in $TOND_SUPPORTED_SYSTEMS; do
        _ds_id="${_ds_entry%%:*}"
        _ds_rest="${_ds_entry#*:}"
        _ds_version="${_ds_rest%%:*}"
        if [ "$_ds_id" = "$SYSTEM_ID" ] && [ "$_ds_version" = "$SYSTEM_VERSION" ]; then
            SYSTEM_SUPPORTED=yes
            [ -n "$SYSTEM_CODENAME" ] || SYSTEM_CODENAME="${_ds_rest#*:}"
        fi
    done
}

# unsupported_system_message — why this system is refused and what to do.
unsupported_system_message() {
    echo "Tond supports ${TOND_SUPPORTED_SYSTEMS_TEXT}. This server runs ${SYSTEM_NAME}."
    echo "Install on a supported system, or rerun with --allow-unsupported-os to try anyway (not supported: things may break)."
}

# docker_apt_target — "<os> <codename>" of Docker's apt repository for this
# system (download.docker.com/linux/<os> <codename>). Derivatives map to the
# release they are based on. Fails when there is no sensible repository.
docker_apt_target() {
    case "$SYSTEM_ID" in
        ubuntu|debian)
            [ -n "$SYSTEM_CODENAME" ] || return 1
            echo "$SYSTEM_ID $SYSTEM_CODENAME"
            ;;
        *)
            case " $SYSTEM_LIKE " in
                *" ubuntu "*)
                    [ -n "$SYSTEM_UBUNTU_CODENAME" ] || return 1
                    echo "ubuntu $SYSTEM_UBUNTU_CODENAME"
                    ;;
                *" debian "*)
                    [ -n "$SYSTEM_DEBIAN_CODENAME" ] || return 1
                    echo "debian $SYSTEM_DEBIAN_CODENAME"
                    ;;
                *) return 1 ;;
            esac
            ;;
    esac
}

# docker_codename_fallback CODENAME — the codename to use when Docker's
# repository doesn't have CODENAME yet (empty when there is none).
docker_codename_fallback() {
    for _dc_pair in $TOND_DOCKER_CODENAME_FALLBACK; do
        if [ "${_dc_pair%%:*}" = "$1" ]; then
            echo "${_dc_pair#*:}"
            return 0
        fi
    done
}
# ── end supported systems ──

# Check if we're in bundled mode or need to download
is_bundled() {
    [[ -f "${SCRIPT_DIR}/tond-server" && -f "${SCRIPT_DIR}/tond-proxy" && -f "${SCRIPT_DIR}/tond" && -d "${SCRIPT_DIR}/dashboard" ]]
}

# Download latest release from GitHub
download_release() {
    step "Downloading Tond release"

    local arch
    arch="$(host_arch)"
    [[ -n "$arch" ]] || fatal "Unsupported architecture: $(uname -m). Tond supports x86_64 (amd64) and aarch64 (arm64)."

    local tmp_dir=$(mktemp -d)
    local tag
    tag="$(latest_release_tag || true)"
    [[ -n "$tag" ]] || fatal "Could not look up the latest Tond release"
    local release_url="https://github.com/tahayvr/tond/releases/download/${tag}/tond-linux-${arch}.tar.gz"
    local checksum_url="${release_url}.sha256"

    info "Downloading tond-linux-${arch} ${tag} from GitHub..."
    curl -fsSL -o "${tmp_dir}/tond.tar.gz" "$release_url" || fatal "Failed to download release"

    info "Verifying checksum..."
    curl -fsSL -o "${tmp_dir}/tond.tar.gz.sha256" "$checksum_url" || fatal "Failed to download checksum"
    # The .sha256 file contains "HASH  filename" — rewrite the filename to match our local path
    local expected_hash
    expected_hash=$(awk '{print $1}' "${tmp_dir}/tond.tar.gz.sha256")
    local actual_hash
    actual_hash=$(sha256sum "${tmp_dir}/tond.tar.gz" | awk '{print $1}')
    if [[ ! "$expected_hash" =~ ^[0-9a-f]{64}$ || "$expected_hash" != "$actual_hash" ]]; then
        fatal "Checksum verification failed! Download may be corrupted or tampered with."
    fi
    info "✓ Checksum verified"

    info "Verifying signature..."
    curl -fsSL -o "${tmp_dir}/tond.tar.gz.minisig" "${release_url}.minisig" 2>/dev/null \
        || rm -f "${tmp_dir}/tond.tar.gz.minisig"
    check_release_signature "$tag" "${tmp_dir}/tond.tar.gz" "${tmp_dir}/tond.tar.gz.minisig" \
        || fatal "Refusing to install an unverified release"

    info "Extracting..."
    tar -xzf "${tmp_dir}/tond.tar.gz" -C "$tmp_dir" || fatal "Failed to extract archive"

    # Find the extracted directory
    local extracted_dir=""
    if [[ -d "${tmp_dir}/tond-linux-${arch}" ]]; then
        extracted_dir="${tmp_dir}/tond-linux-${arch}"
    elif [[ -d "${tmp_dir}/tond" ]]; then
        extracted_dir="${tmp_dir}/tond"
    elif [[ -f "${tmp_dir}/install.sh" ]]; then
        extracted_dir="$tmp_dir"
    else
        # Try to find install.sh
        extracted_dir=$(dirname "$(find "$tmp_dir" -name "install.sh" -type f 2>/dev/null | head -1)")
    fi

    if [[ -z "$extracted_dir" || ! -f "${extracted_dir}/install.sh" ]]; then
        fatal "Could not find extracted files"
    fi

    # Copy files to script directory
    cp -r "${extracted_dir}/"* "$SCRIPT_DIR/" 2>/dev/null || true

    # Cleanup
    rm -rf "$tmp_dir"

    info "Download complete"
}

# Get server's public IP
get_server_ip() {
    curl -fsSL --max-time 5 https://api.ipify.org 2>/dev/null \
        || curl -fsSL --max-time 5 https://ifconfig.me 2>/dev/null \
        || ip addr show | grep 'inet ' | grep -v '127.0.0.1' | awk '{print $2}' | cut -d'/' -f1 | head -1
}

# Is $1 a public IPv4 address (not private, loopback, link-local or CGNAT)?
is_public_ipv4() {
    local ip="$1" a b
    [[ "$ip" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.[0-9]{1,3}\.[0-9]{1,3}$ ]] || return 1
    a=${BASH_REMATCH[1]} b=${BASH_REMATCH[2]}
    ((a == 10 || a == 127 || a == 0 || a >= 224)) && return 1
    ((a == 172 && b >= 16 && b <= 31)) && return 1
    ((a == 192 && b == 168)) && return 1
    ((a == 169 && b == 254)) && return 1
    ((a == 100 && b >= 64 && b <= 127)) && return 1
    return 0
}

# Without a dashboard domain the dashboard is only reachable through an SSH
# tunnel. Default to <ip>.sslip.io, a public wildcard DNS name that resolves to
# the IP it contains, so HTTPS works right after install. Users can switch to
# their own domain later with `tond settings set-dashboard-domain`.
choose_dashboard_domain() {
    [[ -z "$DASHBOARD_DOMAIN" ]] || return 0
    local ip suggestion=""
    ip=$(get_server_ip)
    if is_public_ipv4 "$ip"; then
        suggestion="${ip//./-}.sslip.io"
    fi

    if [[ "$NON_INTERACTIVE" == false && -r /dev/tty ]]; then
        echo "The dashboard is served over HTTPS on a domain that points at this server."
        if [[ -n "$suggestion" ]]; then
            echo "No domain yet? Press Enter to use ${suggestion}, which resolves to this"
            echo "server's IP (you can switch to your own domain later in Settings)."
            read -r -p "Dashboard domain [${suggestion}]: " DASHBOARD_DOMAIN < /dev/tty || DASHBOARD_DOMAIN=""
            DASHBOARD_DOMAIN="${DASHBOARD_DOMAIN:-$suggestion}"
        else
            read -r -p "Dashboard domain (optional, Enter to skip): " DASHBOARD_DOMAIN < /dev/tty || DASHBOARD_DOMAIN=""
        fi
        DASHBOARD_DOMAIN="$(echo "$DASHBOARD_DOMAIN" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
        DASHBOARD_DOMAIN="${DASHBOARD_DOMAIN%.}"
        if [[ -n "$DASHBOARD_DOMAIN" ]] && ! is_valid_domain "$DASHBOARD_DOMAIN"; then
            fatal "Invalid dashboard domain: ${DASHBOARD_DOMAIN}"
        fi
    elif [[ -n "$suggestion" ]]; then
        DASHBOARD_DOMAIN="$suggestion"
        info "No --dashboard-domain given: using ${DASHBOARD_DOMAIN} (resolves to this server)"
        info "Switch to your own domain later with: sudo tond settings set-dashboard-domain <domain>"
    fi
}

# Print banner
print_banner() {
    echo -e "${CYAN}"
    echo "╔════════════════════════════════════╗"
    echo "║           Tond Installer           ║"
    echo "║      Self-Hosted PaaS Platform     ║"
    echo "╚════════════════════════════════════╝"
    echo -e "${NC}"
}

# -------------------------------------------------------------------
# Pre-flight checks
# -------------------------------------------------------------------

# The support matrix: refuse other systems unless explicitly allowed.
check_supported_system() {
    [[ "$(uname -s)" == "Linux" ]] || fatal "Tond runs on Linux servers (${TOND_SUPPORTED_SYSTEMS_TEXT}). This is $(uname -s)."
    detect_system "${TOND_OS_RELEASE:-/etc/os-release}"
    if [[ "$SYSTEM_SUPPORTED" == yes ]]; then
        return 0
    fi
    if [[ "$ALLOW_UNSUPPORTED_OS" == true ]]; then
        warn "${SYSTEM_NAME} is not a supported system (${TOND_SUPPORTED_SYSTEMS_TEXT})."
        warn "Installing anyway because --allow-unsupported-os was given. Things may break, and bug reports for this system may be closed."
        return 0
    fi
    error "$(unsupported_system_message | head -n 1)"
    error "$(unsupported_system_message | tail -n 1)"
    error "Supported systems: https://tond.io/docs/support-matrix"
    exit 1
}

check_prerequisites() {
    step "Checking prerequisites"

    local arch="$(uname -m)"
    local os_name="$(uname -s)"
    local os_id="unknown"
    local os_version="unknown"
    local mem_mb=0
    local disk_avail_gb=0
    local port_80_busy=false
    local port_443_busy=false

    detect_system "${TOND_OS_RELEASE:-/etc/os-release}"
    os_id="${SYSTEM_ID:-unknown}"
    os_version="${SYSTEM_VERSION:-unknown}"

    if command -v free &> /dev/null; then
        mem_mb=$(free -m | awk '/^Mem:/ {print $2}')
    fi

    if command -v df &> /dev/null; then
        disk_avail_gb=$(df -BG / | awk 'NR==2 {gsub(/G/, "", $4); print $4}')
    fi

    if command -v ss &> /dev/null; then
        ss -tlnp | grep -q ':80 ' && port_80_busy=true || true
        ss -tlnp | grep -q ':443 ' && port_443_busy=true || true
    fi

    echo ""
    echo -e "${BOLD}Preflight summary${NC}"
    if [[ $EUID -eq 0 ]]; then
        mark_pass "Running as root"
    else
        mark_fail "Must run as root or with sudo"
    fi

    if [[ "$os_name" == "Linux" ]]; then
        mark_pass "Linux detected"
    else
        mark_fail "Unsupported OS: ${os_name}"
    fi

    if [[ "$SYSTEM_SUPPORTED" == yes ]]; then
        mark_pass "${SYSTEM_NAME} (supported)"
    else
        mark_warn "${SYSTEM_NAME} is not supported; installing anyway (--allow-unsupported-os)"
    fi

    if [[ -n "$(host_arch)" ]]; then
        mark_pass "Architecture ${arch}"
    else
        mark_fail "Unsupported architecture: ${arch} (supported: x86_64, aarch64)"
    fi

    if (( mem_mb >= 4096 )); then
        mark_pass "Memory ${mem_mb} MB available"
    elif (( mem_mb >= 2048 )); then
        mark_warn "Memory ${mem_mb} MB available; 4 GB is recommended"
    elif (( mem_mb > 0 )); then
        mark_warn "Memory ${mem_mb} MB available; at least 2 GB is recommended"
    fi

    if (( disk_avail_gb >= 20 )); then
        mark_pass "Disk space ${disk_avail_gb} GB available on /"
    elif (( disk_avail_gb > 0 )); then
        mark_warn "Disk space ${disk_avail_gb} GB available on /; 20 GB is recommended"
    fi

    if [[ "$port_80_busy" == true ]]; then
        mark_warn "Port 80 is already in use"
    else
        mark_pass "Port 80 available"
    fi

    if [[ "$port_443_busy" == true ]]; then
        mark_warn "Port 443 is already in use"
    else
        mark_pass "Port 443 available"
    fi

    mark_warn "Cloud firewall must allow inbound 80 and 443"
    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        mark_warn "Dashboard domain seed: ${DASHBOARD_DOMAIN} (DNS does not need to be ready before install completes)"
    fi
    if [[ -n "$APPS_DOMAIN" ]]; then
        mark_warn "Apps domain: ${APPS_DOMAIN} (needs a wildcard DNS record *.${APPS_DOMAIN})"
    fi
    if [[ -n "$PREVIEW_DOMAIN" ]]; then
        mark_warn "Preview domain: ${PREVIEW_DOMAIN} (needs a wildcard DNS record *.${PREVIEW_DOMAIN})"
    fi
    echo ""

    # Root check
    if [[ $EUID -ne 0 ]]; then
        fatal "This script must be run as root. Try: curl -fsSL https://tond.io/install | sudo bash"
    fi

    # OS check
    if [[ "$(uname -s)" != "Linux" ]]; then
        fatal "Tond only supports Linux servers. Detected: $(uname -s)"
    fi

    # Architecture check
    if [[ -z "$(host_arch)" ]]; then
        fatal "Unsupported architecture: $arch. Tond supports x86_64 (amd64) and aarch64 (arm64)."
    fi
    if [[ -f "${SCRIPT_DIR}/ARCH" && "$(cat "${SCRIPT_DIR}/ARCH")" != "$(host_arch)" ]]; then
        fatal "This bundle is for $(cat "${SCRIPT_DIR}/ARCH") but this server is $(host_arch). Use tond-linux-$(host_arch).tar.gz."
    fi

    # Check ports
    if command -v ss &> /dev/null; then
        if ss -tlnp | grep -q ':80 '; then
            warn "Port 80 is already in use"
        fi
        if ss -tlnp | grep -q ':443 '; then
            warn "Port 443 is already in use"
        fi
    fi

    info "Prerequisites OK"
}

# -------------------------------------------------------------------
# Installation Steps
# -------------------------------------------------------------------

install_dependencies() {
    step "Installing system dependencies"

    info "Updating package list..."
    apt-get update -qq > /dev/null 2>&1 &
    spinner $!

    info "Installing required packages..."
    apt-get install -y -qq curl git ca-certificates gnupg openssl jq ufw iproute2 procps > /dev/null 2>&1 &
    spinner $!

    info "Dependencies installed"

    # Private-repo clones pass credentials through GIT_CONFIG_COUNT/KEY_n/VALUE_n
    # (never in the URL), which needs git 2.31 or newer.
    local git_version
    git_version=$(git --version | grep -oE '[0-9]+\.[0-9]+' | head -1)
    if ! printf '2.31\n%s\n' "$git_version" | sort -V -C; then
        fatal "git ${git_version} is too old; Tond needs git 2.31 or newer (${TOND_SUPPORTED_SYSTEMS_TEXT} all have it)"
    fi
}

# Docker's apt signing key ("Docker Release (CE deb)", the same for the Debian
# and Ubuntu repositories), as published at
# https://docs.docker.com/engine/install/ubuntu/ — the downloaded key must
# have exactly this fingerprint before apt trusts it.
DOCKER_APT_KEY_FINGERPRINT="9DC858229FC7DD38854AE2D88D81803C0EBFCD88"
DOCKER_APT_KEYRING="/etc/apt/keyrings/docker.asc"

# Install Docker Engine from Docker's apt repository, signed by the key above
# (no `curl | sh` of a convenience script).
install_docker() {
    step "Setting up Docker"

    if command -v docker &> /dev/null; then
        info "Docker already installed: $(docker --version)"
        return 0
    fi

    detect_system "${TOND_OS_RELEASE:-/etc/os-release}"
    local target os_id codename
    target="$(docker_apt_target)" \
        || fatal "Tond installs Docker from Docker's apt repository, which has no packages for ${SYSTEM_NAME}. Install Docker Engine (https://docs.docker.com/engine/install/) and run this installer again."
    os_id="${target%% *}"
    codename="${target#* }"
    # Docker publishes a new distribution release some time after it ships:
    # until then, use the previous release's packages.
    if ! curl -fsI "https://download.docker.com/linux/${os_id}/dists/${codename}/Release" > /dev/null 2>&1; then
        local fallback
        fallback="$(docker_codename_fallback "$codename")"
        if [[ -n "$fallback" ]] \
            && curl -fsI "https://download.docker.com/linux/${os_id}/dists/${fallback}/Release" > /dev/null 2>&1; then
            warn "Docker's apt repository has no ${codename} packages yet; using the ${fallback} ones"
            codename="$fallback"
        else
            fatal "Docker's apt repository has no packages for ${os_id} ${codename} (https://download.docker.com/linux/${os_id}/dists/). Install Docker Engine by hand (https://docs.docker.com/engine/install/) and run this installer again."
        fi
    fi

    info "Adding Docker's apt repository (signed by ${DOCKER_APT_KEY_FINGERPRINT})..."
    install -m 0755 -d "$(dirname "$DOCKER_APT_KEYRING")"
    local key_tmp
    key_tmp="$(mktemp)"
    curl -fsSL "https://download.docker.com/linux/${os_id}/gpg" -o "$key_tmp" \
        || { rm -f "$key_tmp"; fatal "Failed to download Docker's apt signing key"; }
    local fingerprints
    fingerprints="$(gpg --show-keys --with-colons "$key_tmp" 2>/dev/null | awk -F: '$1 == "fpr" { print $10 }')"
    if ! grep -qx "$DOCKER_APT_KEY_FINGERPRINT" <<<"$fingerprints"; then
        rm -f "$key_tmp"
        fatal "Docker's apt signing key has an unexpected fingerprint; refusing to trust it"
    fi
    install -m 0644 "$key_tmp" "$DOCKER_APT_KEYRING"
    rm -f "$key_tmp"
    echo "deb [arch=$(dpkg --print-architecture) signed-by=${DOCKER_APT_KEYRING}] https://download.docker.com/linux/${os_id} ${codename} stable" \
        > /etc/apt/sources.list.d/docker.list

    info "Installing Docker Engine..."
    apt-get update -qq > /dev/null 2>&1 || fatal "apt-get update failed after adding Docker's repository"
    apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin \
        > /dev/null 2>&1 || fatal "Failed to install Docker Engine from Docker's apt repository"
    systemctl enable --now docker > /dev/null 2>&1
    info "Docker installed: $(docker --version)"
}

# Default log rotation for every container on the host. Tond sets rotation on
# each container it starts; this covers everything else (and containers that
# compose files start). Existing settings win: we only fill in missing keys,
# and a non-rotating driver choice (journald, syslog, ...) is left untouched.
DOCKER_DAEMON_JSON="/etc/docker/daemon.json"
configure_docker_daemon() {
    step "Configuring Docker defaults"

    local defaults='{"log-driver":"json-file","log-opts":{"max-size":"10m","max-file":"3"}}'
    local current='{}'
    if [[ -s "$DOCKER_DAEMON_JSON" ]]; then
        if ! current=$(jq -S -c . "$DOCKER_DAEMON_JSON" 2>/dev/null); then
            warn "${DOCKER_DAEMON_JSON} is not valid JSON; leaving it untouched"
            warn "Tond still rotates logs of the containers it starts"
            return 0
        fi
    fi

    local merged
    # icc=false: containers left on Docker's default bridge (Tond never puts
    # any there; see networks.rs) can't reach each other. An explicit
    # setting wins.
    merged=$(jq -S -c --argjson d "$defaults" '
        (if (has("log-driver") and (.["log-driver"] | IN("json-file", "local") | not)) then .
        else .["log-driver"] = (.["log-driver"] // $d["log-driver"])
             | .["log-opts"] = ($d["log-opts"] + (.["log-opts"] // {}))
        end)
        | if has("icc") then . else .icc = false end' <<<"$current")

    if [[ "$merged" == "$current" ]]; then
        info "Docker defaults already configured"
        return 0
    fi

    mkdir -p "$(dirname "$DOCKER_DAEMON_JSON")"
    if [[ -f "$DOCKER_DAEMON_JSON" ]]; then
        cp -p "$DOCKER_DAEMON_JSON" "${DOCKER_DAEMON_JSON}.bak.$(date +%Y%m%d%H%M%S)"
    fi
    jq . <<<"$merged" > "${DOCKER_DAEMON_JSON}.tmp"
    mv "${DOCKER_DAEMON_JSON}.tmp" "$DOCKER_DAEMON_JSON"
    info "Wrote container log rotation and icc=false to ${DOCKER_DAEMON_JSON}"

    # dockerd only reads daemon.json at start. Restart it now, during install,
    # so the defaults apply before any Tond container exists.
    if systemctl is-active --quiet docker; then
        info "Restarting Docker to apply the new defaults..."
        systemctl restart docker
    fi
}

# Cap the systemd journal (tond-server, tond-proxy and everything else log
# there) at 500 MB. A drop-in, so /etc/systemd/journald.conf stays untouched,
# and only when no SystemMaxUse is configured anywhere yet: an operator's
# choice always wins.
JOURNALD_DROPIN_DIR="/etc/systemd/journald.conf.d"
JOURNALD_DROPIN="${JOURNALD_DROPIN_DIR}/tond.conf"
configure_journald() {
    [[ -d /etc/systemd ]] || return 0
    if [[ -f "$JOURNALD_DROPIN" ]]; then
        return 0
    fi
    local existing
    existing=$(grep -lsE '^[[:space:]]*SystemMaxUse[[:space:]]*=' \
        /etc/systemd/journald.conf "${JOURNALD_DROPIN_DIR}"/*.conf 2>/dev/null | head -n1 || true)
    if [[ -n "$existing" ]]; then
        info "Journal size already capped in ${existing}; leaving it"
        return 0
    fi
    mkdir -p "$JOURNALD_DROPIN_DIR"
    printf '%s\n' \
        '# Written by the Tond installer: keep the journal under 500 MB.' \
        '# Delete this file or set SystemMaxUse elsewhere to change it.' \
        '[Journal]' \
        'SystemMaxUse=500M' > "$JOURNALD_DROPIN"
    chmod 0644 "$JOURNALD_DROPIN"
    if command -v systemctl > /dev/null && systemctl is-active --quiet systemd-journald; then
        systemctl restart systemd-journald || warn "Could not restart systemd-journald; the cap applies after the next reboot"
    fi
    info "Capped the systemd journal at 500 MB (${JOURNALD_DROPIN})"
}

create_directories() {
    step "Creating directory structure"

    mkdir -p "${BIN_DIR}" "${DATA_DIR}" "${DATA_DIR}/certs" "${DASHBOARD_DIR}"
    info "Created ${INSTALL_DIR}"
}

# System user tond-proxy runs as: no home, no shell, own group.
ensure_proxy_user() {
    if ! getent group "$PROXY_USER" > /dev/null; then
        groupadd --system "$PROXY_USER"
    fi
    if ! id -u "$PROXY_USER" > /dev/null 2>&1; then
        useradd --system --gid "$PROXY_USER" --no-create-home --home-dir /nonexistent \
            --shell /usr/sbin/nologin "$PROXY_USER"
        info "Created system user ${PROXY_USER}"
    fi
}

# Unprivileged login user for the dashboard's host terminal (TOND_TERMINAL_USER
# in tond-server.service). Deliberately not in the docker or sudo groups: the
# host terminal must not be a path to root. Admins who need root use SSH.
ensure_terminal_user() {
    if ! id -u "$TERMINAL_USER" > /dev/null 2>&1; then
        useradd --create-home --shell /bin/bash --comment "Tond host terminal" "$TERMINAL_USER"
        passwd --lock "$TERMINAL_USER" > /dev/null
        info "Created terminal user ${TERMINAL_USER} (unprivileged)"
    fi
    for group in docker sudo wheel adm; do
        if id -nG "$TERMINAL_USER" | tr ' ' '\n' | grep -qx "$group"; then
            gpasswd -d "$TERMINAL_USER" "$group" > /dev/null
            warn "Removed ${TERMINAL_USER} from the ${group} group (the host terminal must stay unprivileged)"
        fi
    done
}

# Permission scheme for /opt/tond/data (also applied by update.sh):
#   data dir            root:tond-proxy 3770  setgid: new files get group tond-proxy;
#                                             sticky: tond-proxy can create its own
#                                             files (proxy.sock) but never remove or
#                                             replace root's (tond.env, tond.sock, ...)
#   tond.db, -wal, -shm root:tond-proxy 0640  written by tond-server, read-only for
#                                             tond-proxy (which also opens it mode=ro)
#   proxy.sock          tond-proxy 0600       created by tond-proxy at start
#   certs/, acme_account.json  tond-proxy     0700 dirs / 0600 files (TLS + ACME keys)
#   tond.env, setup-token                 root 0600 (written by tond-server/install.sh)
#   everything else     root only: top-level entries lose group/other access;
#                       builds/ compose/ volumes/ databases/ keys/ backups/
#                       docker-config/ are 0700 (the parents are what keep their
#                       contents private — volumes/ and databases/ contents are
#                       container bind mounts and keep their own modes).
secure_data_dir() {
    step "Securing ${DATA_DIR}"

    chown root:"$PROXY_USER" "$DATA_DIR"
    chmod 3770 "$DATA_DIR"

    local dir
    for dir in builds compose volumes databases keys backups docker-config; do
        mkdir -p "${DATA_DIR}/${dir}"
        chown root:root "${DATA_DIR}/${dir}"
        # Symbolic mode: a numeric mode keeps the setgid bit inherited from DATA_DIR.
        chmod u=rwx,go=,g-s "${DATA_DIR}/${dir}"
    done
    # Secrets: safe to tighten recursively (nothing else reads them).
    chmod -R go-rwx "${DATA_DIR}/keys" "${DATA_DIR}/backups" "${DATA_DIR}/docker-config"

    # Everything else at the top level: owner only, except the shared DB and
    # the proxy's own state, handled below.
    find "$DATA_DIR" -mindepth 1 -maxdepth 1 \
        ! -name 'tond.db' ! -name 'tond.db-wal' ! -name 'tond.db-shm' \
        ! -name certs ! -name acme_account.json ! -name proxy.sock ! -name access \
        -exec chmod go-rwx {} +

    # Pre-create the database so the proxy can open it on first start; an
    # empty file is a valid new SQLite database.
    if [[ ! -e "${DATA_DIR}/tond.db" ]]; then
        install -m 0640 -o root -g "$PROXY_USER" /dev/null "${DATA_DIR}/tond.db"
    fi
    local f
    for f in tond.db tond.db-wal tond.db-shm; do
        if [[ -e "${DATA_DIR}/${f}" ]]; then
            chown root:"$PROXY_USER" "${DATA_DIR}/${f}"
            chmod 0640 "${DATA_DIR}/${f}"
        fi
    done

    mkdir -p "${DATA_DIR}/certs"
    chown -R "$PROXY_USER":"$PROXY_USER" "${DATA_DIR}/certs"
    find "${DATA_DIR}/certs" -type d -exec chmod u=rwx,go=,g-s {} +
    find "${DATA_DIR}/certs" -type f -exec chmod 0600 {} +
    # Opt-in per-app access logs: written by the proxy only (tond-server reads
    # them as root and refuses symlinks / hard links, see access_logs.rs).
    # The proxy owns this directory, so never follow a symlink it left there.
    if [[ -L "${DATA_DIR}/access" ]]; then rm -f "${DATA_DIR}/access"; fi
    mkdir -p "${DATA_DIR}/access"
    chown -R "$PROXY_USER":"$PROXY_USER" "${DATA_DIR}/access"
    chmod u=rwx,go=,g-s "${DATA_DIR}/access"
    find "${DATA_DIR}/access" -type f -exec chmod 0600 {} +
    if [[ -e "${DATA_DIR}/acme_account.json" ]]; then
        chown "$PROXY_USER":"$PROXY_USER" "${DATA_DIR}/acme_account.json"
        chmod 0600 "${DATA_DIR}/acme_account.json"
    fi

    if [[ -e "$SECRET_ENV_FILE" ]]; then
        chown root:root "$SECRET_ENV_FILE"
        chmod 0600 "$SECRET_ENV_FILE"
    fi

    info "Data directory locked down (tond.db read-only for ${PROXY_USER} via group; secrets root-only)"
}

# Ensure ${SECRET_ENV_FILE} holds TOND_SECRET_KEY, the at-rest encryption key
# for stored secrets. Never replaces an existing key (reinstalls keep it).
ensure_secret_key() {
    step "Configuring encryption key"

    mkdir -p "${DATA_DIR}"

    if [[ -f "$SECRET_ENV_FILE" ]] && grep -q '^TOND_SECRET_KEY=.' "$SECRET_ENV_FILE"; then
        chown root:root "$SECRET_ENV_FILE"
        chmod 600 "$SECRET_ENV_FILE"
        info "Using existing encryption key from ${SECRET_ENV_FILE}"
        return 0
    fi

    local key=""
    if command -v openssl &> /dev/null; then
        key=$(openssl rand -hex 32)
    else
        key=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
    fi
    [[ ${#key} -eq 64 ]] || fatal "Failed to generate encryption key"
    info "Generated a new encryption key"

    (
        umask 077
        printf 'TOND_SECRET_KEY=%s\n' "$key" > "${SECRET_ENV_FILE}.tmp"
    )
    chown root:root "${SECRET_ENV_FILE}.tmp"
    mv -f "${SECRET_ENV_FILE}.tmp" "$SECRET_ENV_FILE"

    info "Encryption key stored in ${SECRET_ENV_FILE} (mode 0600)"
    warn "Back this file up: without it, stored secrets and remote snapshot copies cannot be decrypted"
}

install_binaries() {
    step "Installing Tond binaries"

    # Check bundled files
    for bin in tond-server tond-proxy tond; do
        if [[ ! -f "${SCRIPT_DIR}/${bin}" ]]; then
            fatal "Missing binary: ${SCRIPT_DIR}/${bin}"
        fi
    done

    if [[ ! -f "${SCRIPT_DIR}/dashboard/index.html" ]]; then
        fatal "Missing dashboard build at ${SCRIPT_DIR}/dashboard/index.html"
    fi

    # Stop existing services if upgrading
    systemctl stop tond-server tond-proxy 2>/dev/null || true

    # Copy binaries
    cp "${SCRIPT_DIR}/tond-server" "${BIN_DIR}/tond-server"
    cp "${SCRIPT_DIR}/tond-proxy"  "${BIN_DIR}/tond-proxy"
    cp "${SCRIPT_DIR}/tond"        "${BIN_DIR}/tond"
    chmod +x "${BIN_DIR}/tond-server" "${BIN_DIR}/tond-proxy" "${BIN_DIR}/tond"

    # Install CLI to PATH
    cp "${BIN_DIR}/tond" /usr/local/bin/tond

    # Copy update script if available
    if [[ -f "${SCRIPT_DIR}/update.sh" ]]; then
        cp "${SCRIPT_DIR}/update.sh" "${BIN_DIR}/update.sh"
        chmod +x "${BIN_DIR}/update.sh"
        info "Update script installed"
    fi

    # Write VERSION file so the update script can detect the installed version
    if [[ -f "${SCRIPT_DIR}/VERSION" ]]; then
        cp "${SCRIPT_DIR}/VERSION" "${INSTALL_DIR}/VERSION"
        info "Version: $(cat "${INSTALL_DIR}/VERSION")"
    fi

    info "Binaries installed to ${BIN_DIR}"
    info "CLI available at /usr/local/bin/tond"
}

install_dashboard() {
    step "Installing dashboard"

    # Remove old dashboard
    rm -rf "${DASHBOARD_DIR:?}/"*

    # Copy new dashboard
    cp -r "${SCRIPT_DIR}/dashboard/." "${DASHBOARD_DIR}/"

    info "Dashboard installed to ${DASHBOARD_DIR}"
}

install_services() {
    step "Installing systemd services"

    # Copy service files
    cp "${SCRIPT_DIR}/tond-server.service" /etc/systemd/system/tond-server.service
    cp "${SCRIPT_DIR}/tond-proxy.service"  /etc/systemd/system/tond-proxy.service
    # Memory cap for image builds (docker build --cgroup-parent tondbuild.slice)
    cp "${SCRIPT_DIR}/tondbuild.slice"    /etc/systemd/system/tondbuild.slice

    systemctl daemon-reload

    info "Services installed"
}

start_services() {
    step "Starting Tond services"

    systemctl enable tond-server tond-proxy > /dev/null 2>&1
    systemctl start tond-server

    # Give the API server a moment to initialize
    sleep 2

    systemctl start tond-proxy

    # Wait for API to be ready
    info "Waiting for API to start..."
    local retries=30
    for i in $(seq 1 $retries); do
        if curl -sf http://localhost:3001/health > /dev/null 2>&1; then
            break
        fi
        sleep 1
    done

    if curl -sf http://localhost:3001/health > /dev/null 2>&1; then
        info "✓ API is ready"
    else
        warn "API did not respond within ${retries} seconds"
        warn "Check logs: journalctl -u tond-server"
    fi
}

configure_dashboard_domain_seed() {
    if [[ -z "$DASHBOARD_DOMAIN" ]]; then
        return 0
    fi

    step "Seeding dashboard domain"

    # tond-server writes the dashboard_domain setting from this on start
    # while the setting is empty (tond-proxy opens the database read-only).
    mkdir -p /etc/systemd/system/tond-server.service.d/

    cat > /etc/systemd/system/tond-server.service.d/dashboard-domain.conf <<EOF
[Service]
Environment=TOND_DASHBOARD_DOMAIN=${DASHBOARD_DOMAIN}
EOF

    systemctl daemon-reload

    info "Dashboard domain seed configured: ${DASHBOARD_DOMAIN}"
}

# Apply the dashboard and preview domains through the API (over the root-only
# CLI socket), so tond-server validates them together — the preview domain
# must be on a different site than the dashboard.
configure_domains() {
    if [[ -z "$PREVIEW_DOMAIN" && -z "$APPS_DOMAIN" ]]; then
        return 0
    fi

    step "Configuring app and preview domains"

    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        tond settings set-dashboard-domain "$DASHBOARD_DOMAIN" > /dev/null \
            || { error "Could not set the dashboard domain"; return 1; }
    fi
    if [[ -n "$PREVIEW_DOMAIN" ]]; then
        if ! tond settings set-preview-domain "$PREVIEW_DOMAIN" > /dev/null; then
            error "tond-server rejected the preview domain ${PREVIEW_DOMAIN}"
            error "Fix it later with: tond settings set-preview-domain <domain>"
            return 1
        fi
        info "Previews will be served at <id>-<app>.${PREVIEW_DOMAIN}"
    fi
    if [[ -n "$APPS_DOMAIN" ]]; then
        if ! tond settings set-apps-domain "$APPS_DOMAIN" > /dev/null; then
            error "tond-server rejected the apps domain ${APPS_DOMAIN}"
            error "Fix it later with: tond settings set-apps-domain <domain>"
            return 1
        fi
        info "Apps will be served at <app>.${APPS_DOMAIN}"
    fi
}

# TCP ports sshd accepts connections on, one per line: the port of the SSH
# session running this installer, sshd's effective configuration, the sockets
# sshd listens on, and systemd's ssh.socket (Ubuntu 22.10+ socket-activates
# sshd, so the listener belongs to systemd, not sshd).
detect_ssh_ports() {
    {
        if [[ -n "${SSH_CONNECTION:-}" ]]; then
            awk '{ print $4 }' <<<"$SSH_CONNECTION"
        fi
        if command -v sshd &> /dev/null; then
            sshd -T 2>/dev/null | awk '$1 == "port" { print $2 }'
        fi
        if command -v ss &> /dev/null; then
            ss -Htlnp 2>/dev/null | awk '/"sshd"/ { n = split($4, a, ":"); print a[n] }'
        fi
        if systemctl list-unit-files ssh.socket &> /dev/null \
            && systemctl is-enabled --quiet ssh.socket 2>/dev/null; then
            systemctl show -p Listen ssh.socket 2>/dev/null | grep -oE ':[0-9]+ ' | tr -d ': '
        fi
    } | grep -E '^[0-9]+$' | awk '$1 > 0 && $1 < 65536' | sort -un
}

# Allow every detected SSH port (22 when none is found) so enabling the
# firewall can't lock the admin out.
allow_ssh_ports() {
    local ports port
    ports="$(detect_ssh_ports)"
    if [[ -z "$ports" ]]; then
        warn "Could not detect sshd's port; allowing 22/tcp"
        ports="22"
    fi
    for port in $ports; do
        ufw allow "${port}/tcp" comment 'SSH' > /dev/null 2>&1
    done
    info "SSH allowed on port(s): $(tr '\n' ' ' <<<"$ports")"
}

configure_firewall() {
    step "Configuring firewall"

    # Check if UFW is active
    if ufw status | grep -q "Status: active"; then
        info "UFW is active, ensuring SSH, 80 and 443 are open"
        allow_ssh_ports
        ufw allow 80/tcp > /dev/null 2>&1
        ufw allow 443/tcp > /dev/null 2>&1
        info "Ports 80 and 443 opened"
    elif command -v ufw &> /dev/null; then
        info "UFW detected but not active"
        local enable_ufw=false
        if [[ "$NON_INTERACTIVE" == true ]]; then
            enable_ufw=true
        else
            read -p "Enable UFW firewall with ports 80/443 open? [Y/n] " -n 1 -r
            echo
            if [[ ! $REPLY =~ ^[Nn]$ ]]; then
                enable_ufw=true
            fi
        fi
        if [[ "$enable_ufw" == true ]]; then
            # Rules first, then enable: SSH must be allowed before the
            # firewall starts dropping connections.
            ufw default deny incoming > /dev/null 2>&1
            ufw default allow outgoing > /dev/null 2>&1
            allow_ssh_ports
            ufw allow 80/tcp > /dev/null 2>&1
            ufw allow 443/tcp > /dev/null 2>&1
            ufw --force enable > /dev/null 2>&1
            info "UFW enabled with SSH, 80 (HTTP) and 443 (HTTPS) open"
        fi
    fi
}

# -------------------------------------------------------------------
# Post-install summary
# -------------------------------------------------------------------

# Wait for tond-server to write its first-run files, then print the setup token
# (empty when the admin account already exists, e.g. on a reinstall).
read_setup_token() {
    local i
    for i in $(seq 1 15); do
        if [[ -s "$SETUP_TOKEN_FILE" ]]; then
            tr -d '[:space:]' < "$SETUP_TOKEN_FILE"
            return 0
        fi
        # Setup already done: the server writes the token (if needed) before it
        # opens its socket, so a listening server without a token file means
        # the admin account exists.
        if [[ -S "${DATA_DIR}/tond.sock" && $i -ge 2 ]]; then
            return 0
        fi
        sleep 1
    done
}

print_summary() {
    echo ""
    echo -e "${GREEN}╔════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║     Installation Complete!       ║${NC}"
    echo -e "${GREEN}╚════════════════════════════════════╝${NC}"
    echo ""

    local server_ip=$(get_server_ip)
    local setup_token
    setup_token="$(read_setup_token || true)"

    echo -e "${BOLD}Server IP:${NC}  ${server_ip}"
    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        echo -e "${BOLD}Dashboard:${NC}  ${CYAN}https://${DASHBOARD_DOMAIN}${NC}"
    fi
    echo ""

    if [[ -n "$setup_token" ]]; then
        echo -e "${YELLOW}${BOLD}Create your admin account${NC}"
        echo "  Only someone with this one-time link can create the admin account, so keep it"
        echo "  private until you have used it:"
        echo ""
        if [[ -n "$DASHBOARD_DOMAIN" ]]; then
            echo -e "  ${CYAN}https://${DASHBOARD_DOMAIN}/setup?token=${setup_token}${NC}"
        else
            echo "  1. Point a DNS A record for your dashboard domain at ${server_ip}"
            echo "  2. sudo tond settings set-dashboard-domain dash.example.com"
            echo -e "  3. Open ${CYAN}https://dash.example.com/setup?token=${setup_token}${NC}"
        fi
        echo ""
        echo "  No DNS yet? Use an SSH tunnel from your computer instead:"
        echo "    ssh -L 3001:127.0.0.1:3001 root@${server_ip}"
        echo -e "    then open ${CYAN}http://localhost:3001/setup?token=${setup_token}${NC}"
        echo ""
        echo "  Print the link again any time with: sudo tond setup-link"
        echo ""
    elif [[ -n "$DASHBOARD_DOMAIN" ]]; then
        echo -e "The admin account already exists: sign in at ${CYAN}https://${DASHBOARD_DOMAIN}${NC}"
        echo ""
    fi

    if [[ "$DASHBOARD_DOMAIN" == *.sslip.io ]]; then
        echo -e "${BOLD}DNS:${NC}        Nothing to do: ${DASHBOARD_DOMAIN} already resolves to this server."
        echo "            To use your own domain: sudo tond settings set-dashboard-domain <domain>"
        echo ""
    elif [[ -n "$DASHBOARD_DOMAIN" ]]; then
        echo -e "${BOLD}DNS:${NC}        Create an A record: ${DASHBOARD_DOMAIN} -> ${server_ip}"
        echo -e "${YELLOW}Note:${NC} HTTPS starts working once DNS points at this server (the certificate"
        echo "      is issued automatically). The dashboard is only served over HTTPS."
        echo ""
    fi

    echo -e "${YELLOW}${BOLD}Back up your encryption key${NC}"
    echo "  ${SECRET_ENV_FILE} holds TOND_SECRET_KEY. Every stored secret (env vars, database"
    echo "  passwords, tokens) and every snapshot copy in remote storage is encrypted with it."
    echo "  Keep a copy somewhere safe off this server: restoring on a new server needs the"
    echo "  same key, and without it that data cannot be recovered."
    echo "  Show it with: sudo cat ${SECRET_ENV_FILE}"
    echo ""

    echo -e "${BOLD}Automation:${NC} Create a scoped API token for CI or scripts (shown once):"
    echo "            sudo tond token create --name ci --preset deployer --app <app> --expires 30d"
    echo ""

    if [[ "$APPS_DOMAIN" == *.nip.io || "$APPS_DOMAIN" == *.sslip.io ]]; then
        echo -e "${BOLD}Apps:${NC}       Served at <app>.${APPS_DOMAIN} (already resolves to this server)."
        echo "            For production, use your own domain on a different site from the"
        echo "            dashboard: tond settings set-apps-domain <domain>"
        echo ""
    elif [[ -n "$APPS_DOMAIN" ]]; then
        echo -e "${BOLD}Apps:${NC}       Create a wildcard A record: *.${APPS_DOMAIN} -> ${server_ip}"
        echo "            Apps are served at <app>.${APPS_DOMAIN}"
        echo ""
    else
        echo -e "${BOLD}Apps:${NC}       No apps domain: give each app a custom domain, or set one with"
        echo "            tond settings set-apps-domain <domain> (a different site from the dashboard)"
        echo ""
    fi

    if [[ -n "$PREVIEW_DOMAIN" ]]; then
        echo -e "${BOLD}Previews:${NC}   Create a wildcard A record: *.${PREVIEW_DOMAIN} -> ${server_ip}"
        echo "            Previews are served at <id>-<app>.${PREVIEW_DOMAIN}"
        echo ""
    else
        echo -e "${BOLD}Previews:${NC}   Disabled until you set a preview domain (a separate domain you own,"
        echo "            with a wildcard DNS record): tond settings set-preview-domain <domain>"
        echo ""
    fi

    echo "Next steps:"
    echo "  1. Open the setup link above and create your admin account"
    echo "  2. Deploy your first app"
    echo ""

    echo "If the dashboard is unreachable, confirm your cloud firewall allows ports 80 and 443."
    echo ""

    echo -e "${BOLD}CLI Commands:${NC}"
    echo "  tond --help          Show all commands (run as root: the CLI uses the root-only socket)"
    echo "  tond apps create my-app --source https://github.com/you/your-app"
    echo "  tond settings host-terminal on   Allow a shell on this server from the dashboard (off by default)"
    echo ""

    echo -e "${BOLD}Update Tond:${NC}"
    echo "  tond update          Update to latest version"
    echo "  tond update --help   See update options"
    echo ""

    echo -e "${BOLD}Service Management:${NC}"
    echo "  systemctl status tond-server tond-proxy"
    echo "  journalctl -fu tond-server"
    echo ""

    echo -e "${BOLD}Documentation:${NC} https://tond.io/docs"
    echo ""
}

verify_installation() {
    step "Verifying installation"

    local failed=false

    # Check services
    for svc in tond-server tond-proxy; do
        if systemctl is-active --quiet "$svc"; then
            info "  ✓ $svc is running"
        else
            error "  ✗ $svc is NOT running"
            failed=true
        fi
    done

    # Check health endpoint
    if curl -sf http://localhost:3001/health > /dev/null 2>&1; then
        info "  ✓ API health check passed"
    else
        error "  ✗ API health check failed"
        failed=true
    fi

    # Check CLI
    if command -v tond &> /dev/null; then
        info "  ✓ CLI is available"
    else
        warn "  ⚠ CLI not in PATH"
    fi

    if [[ "$failed" == true ]]; then
        echo ""
        warn "Some services failed to start. Check the logs:"
        warn "  journalctl -u tond-server --no-pager -n 50"
        warn "  journalctl -u tond-proxy --no-pager -n 50"
        return 1
    fi

    return 0
}

# -------------------------------------------------------------------
# Main
# -------------------------------------------------------------------

main() {
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --non-interactive|-y)
                NON_INTERACTIVE=true
                shift
                ;;
            --dashboard-domain)
                [[ $# -ge 2 ]] || fatal "Missing value for --dashboard-domain"
                DASHBOARD_DOMAIN="$2"
                if ! is_valid_domain "$DASHBOARD_DOMAIN"; then
                    fatal "Invalid dashboard domain: ${DASHBOARD_DOMAIN}"
                fi
                shift 2
                ;;
            --preview-domain)
                [[ $# -ge 2 ]] || fatal "Missing value for --preview-domain"
                PREVIEW_DOMAIN="$(echo "$2" | tr '[:upper:]' '[:lower:]')"
                PREVIEW_DOMAIN="${PREVIEW_DOMAIN%.}"
                shift 2
                ;;
            --apps-domain)
                [[ $# -ge 2 ]] || fatal "Missing value for --apps-domain"
                APPS_DOMAIN="$(echo "$2" | tr '[:upper:]' '[:lower:]')"
                APPS_DOMAIN="${APPS_DOMAIN%.}"
                APPS_DOMAIN_SET=true
                shift 2
                ;;
            --allow-unsupported-os)
                ALLOW_UNSUPPORTED_OS=true
                shift
                ;;
            --help|-h)
                echo "Usage: install.sh [--non-interactive|-y] [--dashboard-domain <domain>] [--apps-domain <domain>] [--preview-domain <domain>] [--allow-unsupported-os]"
                echo ""
                echo "  --non-interactive, -y   Skip all prompts and apply safe defaults"
                echo "                          (enables UFW and prints browser-first next steps)"
                echo "  --dashboard-domain      Dashboard domain (default: <server-ip>.sslip.io when the"
                echo "                          server has a public IPv4 address)"
                echo "  --apps-domain           Domain apps are served under (<app>.<domain>). Must be on a"
                echo "                          different site from the dashboard and preview domains, with"
                echo "                          a wildcard DNS record. Default: <server-ip>.nip.io (or"
                echo "                          .sslip.io when the dashboard is on nip.io). Pass \"\" for"
                echo "                          none (apps then need a custom domain)."
                echo "  --preview-domain        Domain previews are served under (<id>-<app>.<domain>)."
                echo "                          Must be a separate domain you own — not the dashboard"
                echo "                          domain or on the same registrable domain — with a"
                echo "                          wildcard DNS record *.<domain>. Previews stay disabled"
                echo "                          until one is set."
                echo "  --allow-unsupported-os  Install on a Linux distribution outside the support matrix"
                echo "                          (${TOND_SUPPORTED_SYSTEMS_TEXT})."
                echo "                          Not supported: things may break. Also TOND_ALLOW_UNSUPPORTED_OS=1."
                echo ""
                echo "Docs: https://tond.io/docs/quickstart"
                exit 0
                ;;
            *)
                fatal "Unknown argument: $1"
                ;;
        esac
    done

    print_banner

    # Refuse an unsupported system before asking anything or downloading.
    check_supported_system

    choose_dashboard_domain

    if [[ -z "$PREVIEW_DOMAIN" && "$NON_INTERACTIVE" == false && -r /dev/tty ]]; then
        echo "Previews run pull-request code, so they are served on a separate domain you own"
        echo "(not the dashboard's), e.g. example-previews.com with a wildcard DNS record."
        read -r -p "Preview domain (optional, Enter to skip): " PREVIEW_DOMAIN < /dev/tty || PREVIEW_DOMAIN=""
        PREVIEW_DOMAIN="$(echo "$PREVIEW_DOMAIN" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
        PREVIEW_DOMAIN="${PREVIEW_DOMAIN%.}"
    fi
    if [[ -n "$PREVIEW_DOMAIN" ]]; then
        local preview_error
        preview_error="$(check_preview_domain "$PREVIEW_DOMAIN" "$DASHBOARD_DOMAIN")" || fatal "$preview_error"
    fi
    choose_apps_domain
    if [[ -n "$APPS_DOMAIN" ]]; then
        local apps_error
        apps_error="$(check_apps_domain "$APPS_DOMAIN" "$DASHBOARD_DOMAIN" "$PREVIEW_DOMAIN")" || fatal "$apps_error"
    fi

    # Check if we need to download the release
    if ! is_bundled; then
        warn "Running in download mode (binaries not found)"
        download_release
    fi

    # Run installation steps
    check_prerequisites
    install_dependencies
    install_docker
    if [[ "$SYSTEM_SUPPORTED" != yes ]]; then
        mkdir -p "$INSTALL_DIR"
        echo "Installed with --allow-unsupported-os on ${SYSTEM_NAME}" > "$UNSUPPORTED_OS_MARKER"
    else
        rm -f "$UNSUPPORTED_OS_MARKER"
    fi
    configure_docker_daemon
    configure_journald
    ensure_proxy_user
    ensure_terminal_user
    create_directories
    ensure_secret_key
    install_binaries
    install_dashboard
    install_services
    secure_data_dir
    configure_dashboard_domain_seed
    start_services
    local domains_ok=true
    configure_domains || domains_ok=false
    configure_firewall

    # Verify and print summary
    if verify_installation && [[ "$domains_ok" == true ]]; then
        print_summary
    else
        echo ""
        error "Installation completed with errors."
        error "Please check the logs and try again."
        exit 1
    fi
}

# Handle script interruption
trap 'echo; error "Installation interrupted"; exit 130' INT TERM

# Run main function (release/tests/distro-containers.sh sources this file for
# its functions only).
if [[ "${TOND_INSTALL_SOURCE_ONLY:-}" != "1" ]]; then
    main "$@"
fi
