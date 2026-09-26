#!/usr/bin/env bash
set -euo pipefail

# Tond Update Script
# Safely updates Tond to the latest version with zero-downtime deployment
#
# Usage:
#   sudo ./update.sh              # Update to latest version
#   sudo ./update.sh --version 0.2.0  # Update to specific version (without v prefix)
#   sudo ./update.sh --dry-run    # Preview what would be updated

INSTALL_DIR="/opt/tond"
DATA_DIR="${INSTALL_DIR}/data"
BACKUP_DIR="${DATA_DIR}/backups"
SECRET_ENV_FILE="${DATA_DIR}/tond.env"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# Unprivileged user tond-proxy runs as (see tond-proxy.service).
PROXY_USER="tond-proxy"

# Port tond-server listens on — read from environment or fall back to default.
# Override via: TOND_PORT=4000 sudo ./update.sh
TOND_PORT="${TOND_PORT:-3001}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Logging
info()  { echo -e "${GREEN}[update]${NC} $*"; }
warn()  { echo -e "${YELLOW}[update]${NC} $*"; }
error() { echo -e "${RED}[update]${NC} $*" >&2; }
fatal() { error "$@"; exit 1; }
step()  { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }

# Configuration
DRY_RUN=false
VERSION="latest"
SKIP_BACKUP=false
FORCE=false

# Parse arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --version)
                # Normalize: strip any leading "v" so comparisons and VERSION file are consistent
                VERSION="${2#v}"
                shift 2
                ;;
            --skip-backup)
                SKIP_BACKUP=true
                shift
                ;;
            --force)
                FORCE=true
                shift
                ;;
            --help|-h)
                show_help
                exit 0
                ;;
            *)
                error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
    done
}

show_help() {
    cat <<EOF
Tond Update Script

Usage: sudo ./update.sh [OPTIONS]

Options:
  --version VERSION   Update to specific version, without the v prefix (default: latest)
  --dry-run          Preview changes without applying them
  --skip-backup      Skip database backup (not recommended)
  --force            Force update even if already on latest version
  --help, -h         Show this help

Examples:
  sudo ./update.sh                    # Update to latest
  sudo ./update.sh --version 0.2.0    # Update to v0.2.0
  sudo ./update.sh --dry-run          # Preview changes
EOF
}

# Host CPU architecture in bundle naming (tond-linux-<arch>): x86_64 or aarch64.
host_arch() {
    case "$(uname -m)" in
        x86_64|amd64) echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) fatal "Unsupported architecture: $(uname -m). Tond supports x86_64 and aarch64." ;;
    esac
}

# Same permission scheme as install.sh (keep the two copies in sync): see the
# comment above secure_data_dir() in install.sh.
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

# Get current installed version
get_current_version() {
    if [[ -f "${INSTALL_DIR}/VERSION" ]]; then
        cat "${INSTALL_DIR}/VERSION"
    else
        echo "unknown"
    fi
}

# Get latest version from GitHub
get_latest_version() {
    curl -fsSL https://api.github.com/repos/tahayvr/tond/releases/latest | grep '"tag_name":' | sed 's/.*"v\(.*\)".*/\1/'
}

# Download specific version
# Accepts an optional second argument: an existing temp dir to use.
# Prints the path to the extracted package directory (stdout).
download_version() {
    local version="$1"
    local tmp_dir="${2:-$(mktemp -d)}"

    step "Downloading Tond v${version}"

    local arch name url
    arch="$(host_arch)"
    name="tond-linux-${arch}"
    if [[ "$version" == "latest" ]]; then
        url="https://github.com/tahayvr/tond/releases/latest/download/${name}.tar.gz"
    else
        url="https://github.com/tahayvr/tond/releases/download/v${version}/${name}.tar.gz"
    fi

    info "Downloading from GitHub..."
    if [[ "$DRY_RUN" == true ]]; then
        info "[DRY RUN] Would download: $url"
        # Return a placeholder path so callers don't have to special-case dry-run
        echo "${tmp_dir}/${name}"
        return 0
    fi

    if ! curl -fsSL -o "${tmp_dir}/tond.tar.gz" "$url" 2>/dev/null; then
        fatal "Failed to download version ${version}"
    fi

    info "Verifying checksum..."
    if ! curl -fsSL -o "${tmp_dir}/tond.tar.gz.sha256" "${url}.sha256" 2>/dev/null; then
        fatal "Failed to download checksum for version ${version}"
    fi
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

    local extracted="${tmp_dir}/${name}"
    if [[ ! -d "$extracted" ]]; then
        fatal "Expected extracted directory not found: ${extracted}"
    fi

    echo "$extracted"
}

# Create backup
create_backup() {
    if [[ "$SKIP_BACKUP" == true ]]; then
        warn "Skipping backup (--skip-backup specified)"
        return 0
    fi
    
    step "Creating backup"
    
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local backup_name="tond_backup_${timestamp}"
    local backup_path="${BACKUP_DIR}/${backup_name}"
    
    if [[ "$DRY_RUN" == true ]]; then
        info "[DRY RUN] Would create backup at: ${backup_path}"
        return 0
    fi
    
    mkdir -p "$backup_path"
    
    # Backup database
    if [[ -f "${DATA_DIR}/tond.db" ]]; then
        cp "${DATA_DIR}/tond.db" "${backup_path}/tond.db"
        info "✓ Database backed up"
    fi
    
    # Backup current binaries
    if [[ -d "${INSTALL_DIR}/bin" ]]; then
        cp -r "${INSTALL_DIR}/bin" "${backup_path}/bin"
        info "✓ Binaries backed up"
    fi
    
    # Backup dashboard
    if [[ -d "${INSTALL_DIR}/dashboard" ]]; then
        cp -r "${INSTALL_DIR}/dashboard" "${backup_path}/dashboard"
        info "✓ Dashboard backed up"
    fi
    
    # Backup service files
    if [[ -f "/etc/systemd/system/tond-server.service" ]]; then
        cp "/etc/systemd/system/tond-server.service" "${backup_path}/"
        info "✓ tond-server service backed up"
    fi
    if [[ -f "/etc/systemd/system/tond-proxy.service" ]]; then
        cp "/etc/systemd/system/tond-proxy.service" "${backup_path}/"
        info "✓ tond-proxy service backed up"
    fi
    
    # Save backup info
    cat > "${backup_path}/info.txt" <<EOF
Backup created: $(date)
Previous version: $(get_current_version)
EOF
    
    info "Backup created: ${backup_path}"
    echo "${backup_path}"
}

# Pre-update checks
pre_update_checks() {
    step "Running pre-update checks"
    
    # Check root
    if [[ $EUID -ne 0 ]]; then
        fatal "This script must be run as root"
    fi
    
    # Check Tond is installed
    if [[ ! -d "$INSTALL_DIR" ]]; then
        fatal "Tond is not installed at ${INSTALL_DIR}"
    fi
    
    # Check services exist
    if ! systemctl list-unit-files | grep -q tond-server; then
        fatal "Tond services not found. Is Tond installed?"
    fi
    
    # Check for running containers (optional warning)
    if command -v docker &> /dev/null; then
        local running=$(docker ps --filter "label=tond.managed=true" --format "{{.Names}}" 2>/dev/null || true)
        if [[ -n "$running" ]]; then
            warn "Tond-managed containers are running:"
            echo "$running"
            warn "These will be briefly interrupted during the update"
        fi
    fi
    
    info "Pre-update checks passed"
}

# The encryption key is created by install.sh and never generated here: a new
# key would make every stored secret unreadable.
ensure_secret_key() {
    step "Checking encryption key"
    if [[ -f "$SECRET_ENV_FILE" ]] && grep -q '^TOND_SECRET_KEY=.' "$SECRET_ENV_FILE"; then
        chown root:root "$SECRET_ENV_FILE"
        chmod 600 "$SECRET_ENV_FILE"
        info "Using encryption key from ${SECRET_ENV_FILE}"
        return 0
    fi
    fatal "${SECRET_ENV_FILE} with TOND_SECRET_KEY is missing. Restore it from your backup; tond-server cannot start without it."
}

# Perform update with zero-downtime strategy
perform_update() {
    local new_version="$1"
    local download_path="$2"
    
    step "Performing update to v${new_version}"
    
    if [[ "$DRY_RUN" == true ]]; then
        info "[DRY RUN] Would perform the following:"
        info "  1. Stop tond-proxy (external traffic stops)"
        info "  2. Stop tond-server (API goes down briefly)"
        info "  3. Update binaries and dashboard"
        if [[ -f "$SECRET_ENV_FILE" ]]; then
            info "     (keep encryption key in ${SECRET_ENV_FILE})"
        else
            info "     (create encryption key in ${SECRET_ENV_FILE})"
        fi
        info "     (ensure the ${PROXY_USER} user; lock down ${DATA_DIR} permissions)"
        info "  4. Start tond-server (database migrations and secret re-encryption run)"
        info "  5. Start tond-proxy (traffic resumes)"
        info "  6. Verify services are healthy"
        return 0
    fi
    
    # Step 1: Stop proxy first (stops external traffic)
    info "Stopping tond-proxy..."
    systemctl stop tond-proxy
    
    # Step 2: Stop server
    info "Stopping tond-server..."
    systemctl stop tond-server
    
    # Step 3: Update files
    info "Updating binaries..."
    cp "${download_path}/tond-server" "${INSTALL_DIR}/bin/tond-server"
    cp "${download_path}/tond-proxy" "${INSTALL_DIR}/bin/tond-proxy"
    cp "${download_path}/tond" "${INSTALL_DIR}/bin/tond"
    cp "${download_path}/tond" /usr/local/bin/tond
    chmod +x "${INSTALL_DIR}/bin/"*
    
    info "Updating dashboard..."
    rm -rf "${INSTALL_DIR}/dashboard"
    mkdir -p "${INSTALL_DIR}/dashboard"
    cp -r "${download_path}/dashboard/." "${INSTALL_DIR}/dashboard/"
    
    ensure_secret_key

    info "Updating service files..."
    cp "${download_path}/tond-server.service" /etc/systemd/system/tond-server.service
    cp "${download_path}/tond-proxy.service" /etc/systemd/system/tond-proxy.service
    systemctl daemon-reload

    # The new units run tond-proxy as ${PROXY_USER}; fix ownership and modes
    # before either service starts.
    ensure_proxy_user
    secure_data_dir
    
    # Step 4: Start server (runs migrations)
    info "Starting tond-server..."
    systemctl start tond-server
    
    # Wait for server to be ready
    local retries=30
    for i in $(seq 1 $retries); do
        if curl -sf "http://localhost:${TOND_PORT}/health" > /dev/null 2>&1; then
            break
        fi
        sleep 1
    done

    if ! curl -sf "http://localhost:${TOND_PORT}/health" > /dev/null 2>&1; then
        error "Server failed to start after update!"
        error "Check logs: journalctl -u tond-server"
        return 1
    fi
    info "✓ Server is running and healthy"
    
    # Step 5: Start proxy
    info "Starting tond-proxy..."
    systemctl start tond-proxy
    
    # Wait a moment for proxy to initialize
    sleep 2
    
    # Step 6: Verify
    if ! systemctl is-active --quiet tond-proxy; then
        error "Proxy failed to start!"
        error "Check logs: journalctl -u tond-proxy"
        return 1
    fi
    info "✓ Proxy is running"
    
    # Save new version
    echo "$new_version" > "${INSTALL_DIR}/VERSION"
    
    info "Update completed successfully"
}

# Verify update
verify_update() {
    step "Verifying update"
    
    if [[ "$DRY_RUN" == true ]]; then
        info "[DRY RUN] Would verify:"
        info "  - Services are running"
        info "  - API is responding"
        info "  - Dashboard is accessible"
        return 0
    fi
    
    # Check services
    local failed=false
    for svc in tond-server tond-proxy; do
        if systemctl is-active --quiet "$svc"; then
            info "✓ $svc is running"
        else
            error "✗ $svc is NOT running"
            failed=true
        fi
    done
    
    # Check health endpoint
    if curl -sf "http://localhost:${TOND_PORT}/health" > /dev/null 2>&1; then
        info "✓ API health check passed"
    else
        error "✗ API health check failed"
        failed=true
    fi
    
    # Check dashboard file exists
    if [[ -f "${INSTALL_DIR}/dashboard/index.html" ]]; then
        info "✓ Dashboard files present"
    else
        error "✗ Dashboard files missing"
        failed=true
    fi
    
    if [[ "$failed" == true ]]; then
        return 1
    fi
    
    return 0
}

# Cleanup — removes the entire temp directory (parent of the extracted package dir)
cleanup() {
    local tmp_dir="$1"

    if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then
        rm -rf "$tmp_dir"
        info "Cleaned up temporary files"
    fi
}

# Rollback function (in case update fails)
rollback() {
    local backup_path="$1"

    step "Rolling back to previous version"

    warn "Something went wrong. Rolling back..."

    # Stop services
    systemctl stop tond-proxy tond-server 2>/dev/null || true

    # Restore binaries (opt dir)
    if [[ -d "${backup_path}/bin" ]]; then
        cp "${backup_path}/bin/"* "${INSTALL_DIR}/bin/"
    fi

    # Restore the CLI symlink in PATH so the version matches the rolled-back server
    if [[ -f "${backup_path}/bin/tond" ]]; then
        cp "${backup_path}/bin/tond" /usr/local/bin/tond
    fi

    # Restore dashboard
    if [[ -d "${backup_path}/dashboard" ]]; then
        rm -rf "${INSTALL_DIR}/dashboard"
        mkdir -p "${INSTALL_DIR}/dashboard"
        cp -r "${backup_path}/dashboard/." "${INSTALL_DIR}/dashboard/"
    fi

    # Restore database if needed
    if [[ -f "${backup_path}/tond.db" ]]; then
        cp "${backup_path}/tond.db" "${DATA_DIR}/tond.db"
    fi

    # Restore service files
    if [[ -f "${backup_path}/tond-server.service" ]]; then
        cp "${backup_path}/tond-server.service" /etc/systemd/system/
    fi
    if [[ -f "${backup_path}/tond-proxy.service" ]]; then
        cp "${backup_path}/tond-proxy.service" /etc/systemd/system/
    fi
    if [[ -f "${backup_path}/tond-server.service" || -f "${backup_path}/tond-proxy.service" ]]; then
        systemctl daemon-reload
    fi

    # Start services
    systemctl start tond-server
    sleep 2
    systemctl start tond-proxy

    info "Rollback completed"
    info "Previous backup preserved at: ${backup_path}"
}

# Main function
main() {
    parse_args "$@"
    
    # Show header
    echo -e "${CYAN}"
    echo "╔════════════════════════════════════╗"
    echo "║        Tond Update Tool            ║"
    echo "╚════════════════════════════════════╝"
    echo -e "${NC}"
    
    # Check if this is a fresh install
    if [[ ! -d "$INSTALL_DIR" ]]; then
        error "Tond is not installed."
        error "To install, run: curl -fsSL https://tond.io/install | sudo bash"
        exit 1
    fi
    
    # Get current version
    local current_version=$(get_current_version)
    info "Current version: ${current_version}"
    
    # Determine target version
    local target_version
    if [[ "$VERSION" == "latest" ]]; then
        target_version=$(get_latest_version)
        info "Latest version: ${target_version}"
    else
        target_version="$VERSION"
        info "Target version: ${target_version}"
    fi
    
    # Check if already up to date
    if [[ "$current_version" == "$target_version" && "$FORCE" == false && "$DRY_RUN" == false ]]; then
        info "Already on version ${current_version}. Nothing to do."
        info "Use --force to update anyway, or --version to specify a different version."
        exit 0
    fi
    
    # Pre-update checks
    pre_update_checks
    
    # Create backup
    local backup_path=$(create_backup)
    
    # Allocate a single temp directory for the download; pass it to download_version
    # so that cleanup() can remove the whole parent dir (tarball + extracted files).
    local tmp_dir
    tmp_dir=$(mktemp -d)

    # Set trap to cleanup the parent temp dir on exit
    trap 'cleanup "$tmp_dir"' EXIT

    # Download new version (reuses $tmp_dir)
    local download_path
    download_path=$(download_version "$target_version" "$tmp_dir")
    
    # Perform update
    if perform_update "$target_version" "$download_path"; then
        # Verify
        if verify_update; then
            echo ""
            echo -e "${GREEN}╔════════════════════════════════════╗${NC}"
            echo -e "${GREEN}║     Update Successful!             ║${NC}"
            echo -e "${GREEN}╚════════════════════════════════════╝${NC}"
            echo ""
            info "Updated from v${current_version} to v${target_version}"
            info "Backup saved at: ${backup_path}"
            echo ""
            info "Services are running and healthy"
            info "Dashboard is accessible"
        else
            error "Update verification failed"
            rollback "$backup_path"
            exit 1
        fi
    else
        error "Update failed"
        rollback "$backup_path"
        exit 1
    fi
}

# Handle interruption
trap 'echo; error "Update interrupted"; exit 130' INT TERM

# Run main
main "$@"
