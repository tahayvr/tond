#!/usr/bin/env bash
set -euo pipefail

# Tond Installer
# Run on a fresh Ubuntu 22.04/24.04 server (x86_64 or aarch64) as root
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
ROOT_KEY_FILE="${DATA_DIR}/root-api-key"      # root API key for automation
SETUP_TOKEN_FILE="${DATA_DIR}/setup-token"    # one-time admin setup token (removed after setup)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NON_INTERACTIVE=false
DASHBOARD_DOMAIN=""
PREVIEW_DOMAIN=""
# Unprivileged user tond-proxy runs as (see tond-proxy.service); its group
# shares tond.db with tond-server.
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
    local release_url="https://github.com/tahayvr/tond/releases/latest/download/tond-linux-${arch}.tar.gz"
    local checksum_url="${release_url}.sha256"
    
    info "Downloading tond-linux-${arch} from GitHub..."
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

    if [[ -f /etc/os-release ]]; then
        source /etc/os-release
        os_id="${ID:-unknown}"
        os_version="${VERSION_ID:-unknown}"
    fi

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

    if [[ "$os_id" == "ubuntu" && "$os_version" =~ ^(22|24)\. ]]; then
        mark_pass "Ubuntu ${os_version}"
    elif [[ "$os_id" == "ubuntu" ]]; then
        mark_warn "Ubuntu ${os_version} detected; tested on 22.04 and 24.04"
    else
        mark_warn "Detected ${os_id} ${os_version}; Ubuntu is the supported target"
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
    
    # Ubuntu version check (recommended but not enforced)
    if [[ -f /etc/os-release ]]; then
        source /etc/os-release
        if [[ "$ID" != "ubuntu" ]]; then
            warn "Tond is tested on Ubuntu. Detected: $ID"
            warn "Installation may still work, but is not officially supported."
            echo ""
            if [[ "$NON_INTERACTIVE" == false ]]; then
                read -p "Continue anyway? [y/N] " -n 1 -r
                echo
                if [[ ! $REPLY =~ ^[Yy]$ ]]; then
                    exit 1
                fi
            fi
        fi
        
        # Version check
        if [[ "$ID" == "ubuntu" && ! "$VERSION_ID" =~ ^(22|24)\. ]]; then
            warn "Ubuntu $VERSION_ID detected. Tond is tested on 22.04 and 24.04."
        fi
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
    apt-get install -y -qq curl git ca-certificates jq ufw > /dev/null 2>&1 &
    spinner $!
    
    info "Dependencies installed"
}

install_docker() {
    step "Setting up Docker"
    
    if command -v docker &> /dev/null; then
        info "Docker already installed: $(docker --version)"
    else
        info "Installing Docker Engine..."
        curl -fsSL https://get.docker.com | sh > /dev/null 2>&1
        systemctl enable --now docker > /dev/null 2>&1
        info "Docker installed: $(docker --version)"
    fi
}

# Default log rotation for every container on the host. Tond sets rotation on
# each container it starts; this covers everything else (and containers that
# compose files start). Existing settings win: we only fill in missing keys,
# and a non-rotating driver choice (journald, syslog, ...) is left untouched.
DOCKER_DAEMON_JSON="/etc/docker/daemon.json"
configure_docker_daemon() {
    step "Configuring Docker log rotation"

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
    merged=$(jq -S -c --argjson d "$defaults" '
        if (has("log-driver") and (.["log-driver"] | IN("json-file", "local") | not)) then .
        else .["log-driver"] = (.["log-driver"] // $d["log-driver"])
             | .["log-opts"] = ($d["log-opts"] + (.["log-opts"] // {}))
        end' <<<"$current")

    if [[ "$merged" == "$current" ]]; then
        info "Docker log rotation already configured"
        return 0
    fi

    mkdir -p "$(dirname "$DOCKER_DAEMON_JSON")"
    if [[ -f "$DOCKER_DAEMON_JSON" ]]; then
        cp -p "$DOCKER_DAEMON_JSON" "${DOCKER_DAEMON_JSON}.bak.$(date +%Y%m%d%H%M%S)"
    fi
    jq . <<<"$merged" > "${DOCKER_DAEMON_JSON}.tmp"
    mv "${DOCKER_DAEMON_JSON}.tmp" "$DOCKER_DAEMON_JSON"
    info "Wrote container log rotation defaults to ${DOCKER_DAEMON_JSON}"

    # dockerd only reads daemon.json at start. Restart it now, during install,
    # so the defaults apply before any Tond container exists.
    if systemctl is-active --quiet docker; then
        info "Restarting Docker to apply the new defaults..."
        systemctl restart docker
    fi
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

# Permission scheme for /opt/tond/data (also applied by update.sh):
#   data dir            root:tond-proxy 3770  setgid: new files get group tond-proxy;
#                                             sticky: tond-proxy can create files
#                                             (SQLite -wal/-shm) but never remove or
#                                             replace root's (tond.env, tond.sock, ...)
#   tond.db, -wal, -shm root:tond-proxy 0660  shared by tond-server and tond-proxy
#   certs/, acme_account.json  tond-proxy     0700 dirs / 0600 files (TLS + ACME keys)
#   tond.env, root-api-key, setup-token   root 0600 (written by tond-server/install.sh)
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
        ! -name certs ! -name acme_account.json \
        -exec chmod go-rwx {} +

    # Pre-create the database so the proxy can open it on first start; an
    # empty file is a valid new SQLite database.
    if [[ ! -e "${DATA_DIR}/tond.db" ]]; then
        install -m 0660 -o root -g "$PROXY_USER" /dev/null "${DATA_DIR}/tond.db"
    fi
    local f
    for f in tond.db tond.db-wal tond.db-shm; do
        if [[ -e "${DATA_DIR}/${f}" ]]; then
            chown root:"$PROXY_USER" "${DATA_DIR}/${f}"
            chmod 0660 "${DATA_DIR}/${f}"
        fi
    done

    mkdir -p "${DATA_DIR}/certs"
    chown -R "$PROXY_USER":"$PROXY_USER" "${DATA_DIR}/certs"
    find "${DATA_DIR}/certs" -type d -exec chmod u=rwx,go=,g-s {} +
    find "${DATA_DIR}/certs" -type f -exec chmod 0600 {} +
    if [[ -e "${DATA_DIR}/acme_account.json" ]]; then
        chown "$PROXY_USER":"$PROXY_USER" "${DATA_DIR}/acme_account.json"
        chmod 0600 "${DATA_DIR}/acme_account.json"
    fi

    if [[ -e "$SECRET_ENV_FILE" ]]; then
        chown root:root "$SECRET_ENV_FILE"
        chmod 0600 "$SECRET_ENV_FILE"
    fi

    info "Data directory locked down (tond.db shared with ${PROXY_USER} via group; secrets root-only)"
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
    warn "Back this file up: without it, stored credentials, tokens and 2FA secrets cannot be decrypted"
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

    mkdir -p /etc/systemd/system/tond-proxy.service.d/

    cat > /etc/systemd/system/tond-proxy.service.d/override.conf <<EOF
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
    if [[ -z "$PREVIEW_DOMAIN" ]]; then
        return 0
    fi

    step "Configuring preview domain"

    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        tond settings set-dashboard-domain "$DASHBOARD_DOMAIN" > /dev/null \
            || { error "Could not set the dashboard domain"; return 1; }
    fi
    if ! tond settings set-preview-domain "$PREVIEW_DOMAIN"; then
        error "tond-server rejected the preview domain ${PREVIEW_DOMAIN}"
        error "Fix it later with: tond settings set-preview-domain <domain>"
        return 1
    fi
    info "Previews will be served at <id>-<app>.${PREVIEW_DOMAIN}"
}

configure_firewall() {
    step "Configuring firewall"
    
    # Check if UFW is active
    if ufw status | grep -q "Status: active"; then
        info "UFW is active, ensuring ports 80 and 443 are open"
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
            ufw --force enable > /dev/null 2>&1
            ufw default deny incoming > /dev/null 2>&1
            ufw default allow outgoing > /dev/null 2>&1
            ufw allow ssh > /dev/null 2>&1
            ufw allow 80/tcp > /dev/null 2>&1
            ufw allow 443/tcp > /dev/null 2>&1
            info "UFW enabled with ports 22 (SSH), 80 (HTTP), and 443 (HTTPS) open"
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
        # Setup already done: the server removes the token file and never recreates it.
        if [[ -s "$ROOT_KEY_FILE" && $i -ge 5 ]]; then
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

    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        echo -e "${BOLD}DNS:${NC}        Create an A record: ${DASHBOARD_DOMAIN} -> ${server_ip}"
        echo -e "${YELLOW}Note:${NC} HTTPS starts working once DNS points at this server (the certificate"
        echo "      is issued automatically). The dashboard is only served over HTTPS."
        echo ""
    fi

    if [[ -s "$ROOT_KEY_FILE" ]]; then
        echo -e "${BOLD}Root API key${NC} (advanced / automation access) is stored in ${ROOT_KEY_FILE} (root only)."
        echo "  Show it with: sudo cat ${ROOT_KEY_FILE}"
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

    echo -e "${BOLD}Documentation:${NC} https://docs.tond.io"
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
            --help|-h)
                echo "Usage: install.sh [--non-interactive|-y] [--dashboard-domain <domain>] [--preview-domain <domain>]"
                echo ""
                echo "  --non-interactive, -y   Skip all prompts and apply safe defaults"
                echo "                          (enables UFW and prints browser-first next steps)"
                echo "  --dashboard-domain      Seed the dashboard domain for unattended installs"
                echo "  --preview-domain        Domain previews are served under (<id>-<app>.<domain>)."
                echo "                          Must be a separate domain you own — not the dashboard"
                echo "                          domain or on the same registrable domain — with a"
                echo "                          wildcard DNS record *.<domain>. Previews stay disabled"
                echo "                          until one is set."
                exit 0
                ;;
            *)
                fatal "Unknown argument: $1"
                ;;
        esac
    done

    print_banner

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

    # Check if we need to download the release
    if ! is_bundled; then
        warn "Running in download mode (binaries not found)"
        download_release
    fi
    
    # Run installation steps
    check_prerequisites
    install_dependencies
    install_docker
    configure_docker_daemon
    ensure_proxy_user
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

# Run main function
main "$@"
