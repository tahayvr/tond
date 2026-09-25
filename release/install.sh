#!/usr/bin/env bash
set -euo pipefail

# Tond Installer
# Run on a fresh Ubuntu 22.04/24.04 server as root
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
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NON_INTERACTIVE=false
DASHBOARD_DOMAIN=""

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
    
    local tmp_dir=$(mktemp -d)
    local release_url="https://github.com/tahayvr/tond/releases/latest/download/tond-linux-x86_64.tar.gz"
    local checksum_url="${release_url}.sha256"
    
    info "Downloading from GitHub..."
    curl -fsSL -o "${tmp_dir}/tond.tar.gz" "$release_url" || fatal "Failed to download release"
    
    info "Verifying checksum..."
    curl -fsSL -o "${tmp_dir}/tond.tar.gz.sha256" "$checksum_url" || fatal "Failed to download checksum"
    # The .sha256 file contains "HASH  filename" — rewrite the filename to match our local path
    local expected_hash
    expected_hash=$(awk '{print $1}' "${tmp_dir}/tond.tar.gz.sha256")
    local actual_hash
    actual_hash=$(sha256sum "${tmp_dir}/tond.tar.gz" | awk '{print $1}')
    if [[ "$expected_hash" != "$actual_hash" ]]; then
        fatal "Checksum verification failed! Download may be corrupted or tampered with."
    fi
    info "✓ Checksum verified"
    
    info "Extracting..."
    tar -xzf "${tmp_dir}/tond.tar.gz" -C "$tmp_dir" || fatal "Failed to extract archive"
    
    # Find the extracted directory
    local extracted_dir=""
    if [[ -d "${tmp_dir}/tond" ]]; then
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

    if [[ "$arch" == "x86_64" ]]; then
        mark_pass "Architecture ${arch}"
    else
        mark_fail "Unsupported architecture: ${arch}"
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
    if [[ "$arch" != "x86_64" ]]; then
        fatal "Unsupported architecture: $arch. Tond requires x86_64 (amd64)."
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

create_directories() {
    step "Creating directory structure"
    
    mkdir -p "${BIN_DIR}" "${DATA_DIR}" "${DATA_DIR}/certs" "${DASHBOARD_DIR}"
    info "Created ${INSTALL_DIR}"
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

print_summary() {
    echo ""
    echo -e "${GREEN}╔════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║     Installation Complete!       ║${NC}"
    echo -e "${GREEN}╚════════════════════════════════════╝${NC}"
    echo ""

    local server_ip=$(get_server_ip)
    local dashboard_url="http://${server_ip}"

    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        dashboard_url="https://${DASHBOARD_DOMAIN}"
    fi

    # Get root API key
    # The key line is a tracing log line (timestamp, level, target); keep only the 64-hex key.
    local root_key=$(journalctl -u tond-server --no-pager | grep -A 1 "ROOT API KEY" | tail -1 | grep -oE '[0-9a-f]{64}')
    if [[ -n "$root_key" ]]; then
        echo -e "${YELLOW}${BOLD}Root API Key (advanced / automation access):${NC}"
        echo "  $root_key"
        echo "  Recover later with: journalctl -u tond-server --no-pager | grep \"ROOT API KEY\" -A 1"
        echo ""
    fi

    echo -e "${BOLD}Server IP:${NC}  ${server_ip}"
    echo -e "${BOLD}Dashboard:${NC}  ${CYAN}${dashboard_url}${NC}"
    echo ""

    if [[ -n "$DASHBOARD_DOMAIN" ]]; then
        echo -e "${BOLD}DNS:${NC}        Create an A record: ${DASHBOARD_DOMAIN} -> ${server_ip}"
        echo -e "${YELLOW}Note:${NC} HTTPS will start working after DNS points at this server."
        echo -e "${YELLOW}Note:${NC} If DNS is not ready yet, you can still use:"
        echo "  http://${server_ip}"
        echo ""
    fi

    echo "Next steps:"
    echo "  1. Open the dashboard URL in your browser"
    echo "  2. Create your admin account"
    echo "  3. Configure your dashboard domain in Settings if needed"
    echo "  4. Deploy your first app"
    echo ""

    echo "If the dashboard is unreachable, confirm your cloud firewall allows ports 80 and 443."
    echo ""

    echo -e "${BOLD}CLI Commands:${NC}"
    echo "  tond --help          Show all commands (run as root: the CLI uses the root-only socket)"
    echo "  tond apps create my-app --source https://github.com/you/your-app"
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
            --help|-h)
                echo "Usage: install.sh [--non-interactive|-y] [--dashboard-domain <domain>]"
                echo ""
                echo "  --non-interactive, -y   Skip all prompts and apply safe defaults"
                echo "                          (enables UFW and prints browser-first next steps)"
                echo "  --dashboard-domain      Seed the dashboard domain for unattended installs"
                exit 0
                ;;
            *)
                fatal "Unknown argument: $1"
                ;;
        esac
    done

    print_banner
    
    # Check if we need to download the release
    if ! is_bundled; then
        warn "Running in download mode (binaries not found)"
        download_release
    fi
    
    # Run installation steps
    check_prerequisites
    install_dependencies
    install_docker
    create_directories
    ensure_secret_key
    install_binaries
    install_dashboard
    install_services
    configure_dashboard_domain_seed
    start_services
    configure_firewall
    
    # Verify and print summary
    if verify_installation; then
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
