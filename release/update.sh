#!/usr/bin/env bash
set -euo pipefail

# Tond update script — updates Tond in place, and rolls back on failure.
#
#   sudo ./update.sh                       # update to the latest release
#   sudo ./update.sh --version 0.4.0       # update to a specific release
#   sudo ./update.sh --bundle ./tond-linux-x86_64   # install an extracted bundle
#   sudo ./update.sh --rollback            # back to the version before the last update
#   sudo ./update.sh --dry-run             # show what would happen
#
# Usually you don't run this yourself: the dashboard (System → Updates) and
# `tond update` ask tond-server, which takes a full snapshot first and then
# runs `tond update --direct` in a transient systemd unit (tond-update.service);
# that verifies the release's checksum and minisign signature and runs the
# release's own update.sh with --bundle.
#
# What an update does:
#   download   fetch the release, verify checksum + signature (not with --bundle)
#   backup     copy the current binaries, dashboard and units to
#              <data>/backups/tond_backup_<time>/ (info.env describes it)
#   install    stop tond-server, copy tond.db into the backup (consistent:
#              nothing writes it while the server is stopped), swap in the
#              new binaries, dashboard and units
#   migrate    start tond-server (database migrations run) and wait until it
#              is healthy and reports the new version
#   proxy      restart tond-proxy (apps are unreachable for a few seconds)
#   done       record the new version, keep the three newest backups
#
# If anything after "install" fails, the old binaries, dashboard and units go
# back, and so does the old database when migrations may have run; then the
# old version starts again ("rolled_back"). Nothing is lost: the server was
# stopped between the database copy and the failure.
#
# --rollback undoes the last update for 7 days: it takes a backup of the
# current version the same way, installs the previous one from the update's
# backup and restores the pre-update database if that update migrated it
# (changes made since are lost — the dashboard and `tond update --rollback`
# take a snapshot first). A failed rollback puts the current version back.
#
# Progress goes to <data>/update/status.json (read by GET /api/system/update)
# and to stdout (appended to <data>/update/update.log by the unit).
#
# Exit codes: 0 updated / rolled back / nothing to do, 1 failed (nothing
# changed, or the automatic rollback restored the previous version — see the
# status), 2 usage error, 3 the automatic rollback failed too (Tond may be
# down; the message says how to recover), 130 interrupted.

INSTALL_DIR="${TOND_INSTALL_DIR:-/opt/tond}"
BIN_DIR="${INSTALL_DIR}/bin"
DATA_DIR="${INSTALL_DIR}/data"
BACKUP_DIR="${DATA_DIR}/backups"
UPDATE_DIR="${DATA_DIR}/update"
STATUS_FILE="${UPDATE_DIR}/status.json"
SECRET_ENV_FILE="${DATA_DIR}/tond.env"
SOCKET_PATH="${DATA_DIR}/tond.sock"
SYSTEMD_DIR="${TOND_SYSTEMD_DIR:-/etc/systemd/system}"
CLI_PATH="${TOND_CLI_PATH:-/usr/local/bin/tond}"
# Written by install.sh --allow-unsupported-os: updates warn instead of refusing.
UNSUPPORTED_OS_MARKER="${INSTALL_DIR}/ALLOW_UNSUPPORTED_OS"
# Unprivileged users: the host terminal's and tond-proxy's (see the units).
TERMINAL_USER="tond"
PROXY_USER="tond-proxy"

# Port tond-server listens on. Override via: TOND_PORT=4000 sudo ./update.sh
TOND_PORT="${TOND_PORT:-3001}"
# How long tond-server gets to become healthy after a start (migrations run first).
HEALTH_TIMEOUT="${TOND_UPDATE_HEALTH_TIMEOUT:-90}"
# How long "Roll back to the previous version" is offered.
ROLLBACK_WINDOW_SECS=$((7 * 24 * 3600))
# Update backups kept (the newest; older ones are deleted after a success).
KEEP_BACKUPS=3

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m' GREEN='\033[0;32m' YELLOW='\033[1;33m' CYAN='\033[0;36m' BOLD='\033[1m' NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' NC=''
fi

info()  { echo -e "${GREEN}[update]${NC} $*"; }
warn()  { echo -e "${YELLOW}[update]${NC} $*"; }
error() { echo -e "${RED}[update]${NC} $*" >&2; }
step_banner() { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }

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

# ── Arguments ──
DRY_RUN=false
VERSION="latest"
BUNDLE_DIR=""
FORCE=false
MODE="update"
ROLLBACK_BACKUP=""
RUN_ID=""
ACTOR=""
SNAPSHOT_ID=""
ALLOW_UNSUPPORTED_OS=false

show_help() {
    cat <<EOF
Tond update script

Usage: sudo ./update.sh [OPTIONS]

Options:
  --version VERSION        Update to this release, without the v prefix (default: latest)
  --bundle DIR             Install an already extracted and verified release bundle
  --rollback               Go back to the version before the last update (within 7 days)
  --backup DIR             With --rollback: the update backup to roll back to
  --force                  Reinstall even when that version is already installed
  --dry-run                Show what would happen without changing anything
  --allow-unsupported-os   Run on a Linux distribution outside the support matrix
  --help, -h               Show this help

The dashboard (System → Updates) and \`tond update\` take a full snapshot first
and then run this script; prefer them.

Examples:
  sudo ./update.sh                    # update to the latest release
  sudo ./update.sh --version 0.4.0    # update to v0.4.0
  sudo ./update.sh --rollback         # undo the last update
EOF
}

usage_error() {
    error "$1"
    echo "Run with --help for usage." >&2
    exit 2
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --dry-run) DRY_RUN=true; shift ;;
            --version)
                [[ $# -ge 2 && -n "$2" ]] || usage_error "--version needs a value"
                VERSION="${2#v}"; shift 2 ;;
            --bundle)
                [[ $# -ge 2 && -n "$2" ]] || usage_error "--bundle needs a directory"
                BUNDLE_DIR="$2"; shift 2 ;;
            --force) FORCE=true; shift ;;
            --rollback) MODE="rollback"; shift ;;
            --backup)
                [[ $# -ge 2 && -n "$2" ]] || usage_error "--backup needs a directory"
                ROLLBACK_BACKUP="$2"; shift 2 ;;
            --run-id)
                [[ $# -ge 2 ]] || usage_error "--run-id needs a value"
                RUN_ID="$2"; shift 2 ;;
            --actor)
                [[ $# -ge 2 ]] || usage_error "--actor needs a value"
                ACTOR="$2"; shift 2 ;;
            --snapshot-id)
                [[ $# -ge 2 ]] || usage_error "--snapshot-id needs a value"
                SNAPSHOT_ID="$2"; shift 2 ;;
            --allow-unsupported-os) ALLOW_UNSUPPORTED_OS=true; shift ;;
            --help|-h) show_help; exit 0 ;;
            *) usage_error "Unknown option: $1" ;;
        esac
    done
    if [[ -n "$ROLLBACK_BACKUP" && "$MODE" != "rollback" ]]; then
        usage_error "--backup only goes with --rollback"
    fi
    if [[ "$MODE" == "rollback" && ( -n "$BUNDLE_DIR" || "$VERSION" != "latest" ) ]]; then
        usage_error "--rollback can't be combined with --version or --bundle"
    fi
}

# ── Status (status.json, read by tond-server) ──

RUN_KIND="update"
FROM_VERSION=""
TO_VERSION=""
STARTED_AT=""
DB_RESTORED=false
CURRENT_STEP=""

# json_string VALUE — VALUE as a JSON string literal.
json_string() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    printf '"%s"' "$s"
}

# write_status STATE STEP MESSAGE — replace status.json (atomically).
write_status() {
    local state="$1" step="$2" message="$3" now finished="null" snapshot="null"
    CURRENT_STEP="$step"
    [[ "$DRY_RUN" == true ]] && return 0
    now="$(date +%s)"
    [[ "$state" == "running" ]] || finished="$now"
    [[ -z "$SNAPSHOT_ID" ]] || snapshot="$(json_string "$SNAPSHOT_ID")"
    mkdir -p "$UPDATE_DIR"
    chmod 700 "$UPDATE_DIR" 2>/dev/null || true
    {
        printf '{\n'
        printf '  "id": %s,\n' "$(json_string "$RUN_ID")"
        printf '  "kind": %s,\n' "$(json_string "$RUN_KIND")"
        printf '  "state": %s,\n' "$(json_string "$state")"
        printf '  "step": %s,\n' "$(json_string "$step")"
        printf '  "from_version": %s,\n' "$(json_string "$FROM_VERSION")"
        printf '  "to_version": %s,\n' "$(json_string "$TO_VERSION")"
        printf '  "actor": %s,\n' "$(json_string "$ACTOR")"
        printf '  "snapshot_id": %s,\n' "$snapshot"
        printf '  "started_at": %s,\n' "$STARTED_AT"
        printf '  "updated_at": %s,\n' "$now"
        printf '  "finished_at": %s,\n' "$finished"
        printf '  "message": %s,\n' "$(json_string "$message")"
        printf '  "pid": %s,\n' "$$"
        printf '  "database_restored": %s\n' "$DB_RESTORED"
        printf '}\n'
    } > "${STATUS_FILE}.tmp"
    mv -f "${STATUS_FILE}.tmp" "$STATUS_FILE"
}

# step NAME MESSAGE — start a step: banner + status.
step() {
    step_banner "$2"
    write_status running "$1" "$2"
}

# finish STATE MESSAGE EXIT_CODE — final status, then exit.
finish() {
    write_status "$1" done "$2"
    case "$1" in
        succeeded) info "$2" ;;
        *) error "$2" ;;
    esac
    exit "$3"
}

# fail_unchanged MESSAGE — a failure before anything was changed.
fail_unchanged() {
    finish failed "$1 Nothing was changed." 1
}

# ── Helpers ──

host_arch() {
    case "$(uname -m)" in
        x86_64|amd64) echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) echo "" ;;
    esac
}

# version_of FILE — a VERSION file's content without the v prefix.
version_of() {
    local v=""
    [[ -f "$1" ]] && v="$(head -n 1 "$1" | tr -d '[:space:]')"
    echo "${v#v}"
}

installed_version() {
    local v
    v="$(version_of "${INSTALL_DIR}/VERSION")"
    echo "${v:-unknown}"
}

# api_field NAME — a numeric or string field of GET /api/system/update from
# the running tond-server, over the root-only socket (empty when unavailable).
api_field() {
    local body
    body="$(curl -sf --max-time 5 --unix-socket "$SOCKET_PATH" http://tond/api/system/update 2>/dev/null || true)"
    printf '%s' "$body" | sed -n "s/.*\"$1\":\"\{0,1\}\([^\",}]*\)\"\{0,1\}[,}].*/\1/p" | head -n 1
}

server_healthy() {
    curl -sf --max-time 5 "http://localhost:${TOND_PORT}/health" > /dev/null 2>&1
}

# wait_for_server EXPECTED_VERSION — until tond-server answers /health and
# (when it reports one) runs EXPECTED_VERSION; fails after HEALTH_TIMEOUT.
wait_for_server() {
    local expected="$1" i reported
    for ((i = 0; i < HEALTH_TIMEOUT; i++)); do
        if ! systemctl is-active --quiet tond-server && ((i > 5)); then
            # Crashed (e.g. a failed migration); systemd would restart it
            # later, but there's no point waiting.
            systemctl is-failed --quiet tond-server && return 1
        fi
        if server_healthy; then
            reported="$(api_field current_version)"
            if [[ -z "$reported" || -z "$expected" || "$reported" == "$expected" ]]; then
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

wait_for_proxy() {
    local i
    for ((i = 0; i < 15; i++)); do
        systemctl is-active --quiet tond-proxy && return 0
        sleep 1
    done
    return 1
}

# db_restore_needed SCHEMA_BEFORE SCHEMA_AFTER — succeeds (restore the
# database copy) unless both schema versions are known and equal: an older
# tond-server refuses a database migrated by a newer one.
db_restore_needed() {
    [[ -z "$1" || -z "$2" || "$1" != "$2" ]]
}

# migrated_flag SCHEMA_BEFORE SCHEMA_AFTER — info.env's MIGRATED value.
migrated_flag() {
    if [[ -z "$1" || -z "$2" ]]; then
        echo "unknown"
    elif [[ "$1" == "$2" ]]; then
        echo "0"
    else
        echo "1"
    fi
}

# info_value FILE KEY — a value from a backup's info.env.
info_value() {
    [[ -f "$1" ]] || return 0
    sed -n "s/^$2=//p" "$1" | head -n 1
}

# set_info FILE KEY VALUE — set KEY in info.env.
set_info() {
    local file="$1" key="$2" value="$3" tmp
    tmp="${file}.tmp"
    { grep -v "^${key}=" "$file" 2>/dev/null || true; echo "${key}=${value}"; } > "$tmp"
    mv -f "$tmp" "$file"
}

# src_path SRC NAME — NAME inside a release bundle (files at the top) or a
# backup (binaries under bin/).
src_path() {
    if [[ -e "$1/bin/$2" ]]; then
        echo "$1/bin/$2"
    else
        echo "$1/$2"
    fi
}

# replace_file SRC DEST MODE — copy next to DEST and rename over it, so a
# running binary is replaced, never overwritten in place ("text file busy").
replace_file() {
    local src="$1" dest="$2" mode="$3"
    cp "$src" "${dest}.new"
    chmod "$mode" "${dest}.new"
    mv -f "${dest}.new" "$dest"
}

# ── Same permission scheme as install.sh (keep the copies in sync; see the
# comment above secure_data_dir() in install.sh) ──

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

ensure_terminal_user() {
    if ! id -u "$TERMINAL_USER" > /dev/null 2>&1; then
        useradd --create-home --shell /bin/bash --comment "Tond host terminal" "$TERMINAL_USER"
        passwd --lock "$TERMINAL_USER" > /dev/null
        info "Created terminal user ${TERMINAL_USER} (unprivileged)"
    fi
    local group
    for group in docker sudo wheel adm; do
        if id -nG "$TERMINAL_USER" | tr ' ' '\n' | grep -qx "$group"; then
            gpasswd -d "$TERMINAL_USER" "$group" > /dev/null
            warn "Removed ${TERMINAL_USER} from the ${group} group (the host terminal must stay unprivileged)"
        fi
    done
}

secure_data_dir() {
    chown root:"$PROXY_USER" "$DATA_DIR"
    chmod 3770 "$DATA_DIR"

    local dir
    for dir in builds compose volumes databases keys backups docker-config update; do
        mkdir -p "${DATA_DIR}/${dir}"
        chown root:root "${DATA_DIR}/${dir}"
        # Symbolic mode: a numeric mode keeps the setgid bit inherited from DATA_DIR.
        chmod u=rwx,go=,g-s "${DATA_DIR}/${dir}"
    done
    chmod -R go-rwx "${DATA_DIR}/keys" "${DATA_DIR}/backups" "${DATA_DIR}/docker-config" "${DATA_DIR}/update"

    find "$DATA_DIR" -mindepth 1 -maxdepth 1 \
        ! -name 'tond.db' ! -name 'tond.db-wal' ! -name 'tond.db-shm' \
        ! -name certs ! -name acme_account.json ! -name proxy.sock ! -name access \
        -exec chmod go-rwx {} +

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
}

# Cap the systemd journal at 500 MB (same as install.sh): a drop-in, only
# when no SystemMaxUse is configured anywhere yet.
JOURNALD_DROPIN_DIR="/etc/systemd/journald.conf.d"
JOURNALD_DROPIN="${JOURNALD_DROPIN_DIR}/tond.conf"
configure_journald() {
    [[ -d /etc/systemd && -z "${TOND_SYSTEMD_DIR:-}" ]] || return 0
    [[ -f "$JOURNALD_DROPIN" ]] && return 0
    local existing
    existing=$(grep -lsE '^[[:space:]]*SystemMaxUse[[:space:]]*=' \
        /etc/systemd/journald.conf "${JOURNALD_DROPIN_DIR}"/*.conf 2>/dev/null | head -n1 || true)
    [[ -z "$existing" ]] || return 0
    mkdir -p "$JOURNALD_DROPIN_DIR"
    printf '%s\n' \
        '# Written by the Tond installer: keep the journal under 500 MB.' \
        '# Delete this file or set SystemMaxUse elsewhere to change it.' \
        '[Journal]' \
        'SystemMaxUse=500M' > "$JOURNALD_DROPIN"
    chmod 0644 "$JOURNALD_DROPIN"
    if systemctl is-active --quiet systemd-journald; then
        systemctl restart systemd-journald || warn "Could not restart systemd-journald; the cap applies after the next reboot"
    fi
    info "Capped the systemd journal at 500 MB (${JOURNALD_DROPIN})"
}

# ── Preflight ──

preflight() {
    if [[ "$(id -u)" != "0" ]]; then
        fail_unchanged "update.sh must run as root (try: sudo $0)."
    fi
    if [[ ! -d "$BIN_DIR" || ! -f "${BIN_DIR}/tond-server" ]]; then
        fail_unchanged "Tond is not installed at ${INSTALL_DIR}. To install: curl -fsSL https://tond.io/install | sudo bash"
    fi
    if [[ ! -f "${SYSTEMD_DIR}/tond-server.service" ]]; then
        fail_unchanged "${SYSTEMD_DIR}/tond-server.service is missing. Reinstall Tond."
    fi
    if [[ ! -f "$SECRET_ENV_FILE" ]] || ! grep -q '^TOND_SECRET_KEY=.' "$SECRET_ENV_FILE"; then
        fail_unchanged "${SECRET_ENV_FILE} with TOND_SECRET_KEY is missing; tond-server can't start without it. Restore it from your backup."
    fi
    if [[ -z "$(host_arch)" ]]; then
        fail_unchanged "Unsupported architecture $(uname -m): Tond runs on x86_64 and aarch64."
    fi
    detect_system "${TOND_OS_RELEASE:-/etc/os-release}"
    if [[ "$SYSTEM_SUPPORTED" != yes ]]; then
        if [[ "$ALLOW_UNSUPPORTED_OS" == true || -f "$UNSUPPORTED_OS_MARKER" ]]; then
            warn "${SYSTEM_NAME} is not a supported system (${TOND_SUPPORTED_SYSTEMS_TEXT}); continuing because it was explicitly allowed."
        else
            fail_unchanged "$(unsupported_system_message | tr '\n' ' ')"
        fi
    fi
}

# ── Download ──

DOWNLOAD_TMP=""
cleanup() {
    if [[ -n "$DOWNLOAD_TMP" && -d "$DOWNLOAD_TMP" ]]; then
        rm -rf "$DOWNLOAD_TMP"
    fi
}
trap cleanup EXIT

# download_release VERSION — sets BUNDLE_DIR to the verified, extracted bundle.
download_release() {
    local version="$1" arch name url
    arch="$(host_arch)"
    name="tond-linux-${arch}"
    url="https://github.com/tahayvr/tond/releases/download/v${version}/${name}.tar.gz"
    if [[ "$DRY_RUN" == true ]]; then
        info "[dry run] Would download ${url} and verify its checksum and signature"
        BUNDLE_DIR="(download)"
        return 0
    fi
    DOWNLOAD_TMP="$(mktemp -d)"
    info "Downloading ${url}"
    curl -fsSL -o "${DOWNLOAD_TMP}/tond.tar.gz" "$url" \
        || fail_unchanged "Could not download Tond ${version} (${url})."
    curl -fsSL -o "${DOWNLOAD_TMP}/tond.tar.gz.sha256" "${url}.sha256" \
        || fail_unchanged "Could not download the checksum of Tond ${version}."
    local expected actual
    expected="$(awk '{print $1}' "${DOWNLOAD_TMP}/tond.tar.gz.sha256")"
    actual="$(sha256sum "${DOWNLOAD_TMP}/tond.tar.gz" | awk '{print $1}')"
    if [[ ! "$expected" =~ ^[0-9a-f]{64}$ || "$expected" != "$actual" ]]; then
        fail_unchanged "The download's checksum doesn't match: it may be corrupted or tampered with."
    fi
    info "Checksum verified"
    curl -fsSL -o "${DOWNLOAD_TMP}/tond.tar.gz.minisig" "${url}.minisig" 2>/dev/null \
        || rm -f "${DOWNLOAD_TMP}/tond.tar.gz.minisig"
    check_release_signature "v${version}" "${DOWNLOAD_TMP}/tond.tar.gz" "${DOWNLOAD_TMP}/tond.tar.gz.minisig" \
        || fail_unchanged "Refusing to install an unverified release."
    tar -xzf "${DOWNLOAD_TMP}/tond.tar.gz" -C "$DOWNLOAD_TMP" \
        || fail_unchanged "Could not extract the release."
    BUNDLE_DIR="${DOWNLOAD_TMP}/${name}"
    [[ -d "$BUNDLE_DIR" ]] || fail_unchanged "The release has no ${name}/ directory."
}

# check_bundle DIR — a complete release bundle for this machine.
check_bundle() {
    local dir="$1" f
    for f in tond-server tond-proxy tond tond-server.service tond-proxy.service dashboard/index.html; do
        [[ -e "${dir}/${f}" ]] || fail_unchanged "The release bundle ${dir} is incomplete: ${f} is missing."
    done
    if [[ -f "${dir}/ARCH" && "$(head -n 1 "${dir}/ARCH")" != "$(host_arch)" ]]; then
        fail_unchanged "The bundle is for $(head -n 1 "${dir}/ARCH") but this server is $(host_arch)."
    fi
}

# ── Backups ──

BACKUP_PATH=""

# create_backup KIND — copy the installed binaries, dashboard and units into
# a new backup directory (the database is added once tond-server is stopped).
create_backup() {
    local kind="$1" stamp
    stamp="$(date +%Y%m%d_%H%M%S)"
    BACKUP_PATH="${BACKUP_DIR}/tond_backup_${stamp}"
    local n=1
    while [[ -e "$BACKUP_PATH" ]]; do
        BACKUP_PATH="${BACKUP_DIR}/tond_backup_${stamp}_${n}"
        n=$((n + 1))
    done
    if [[ "$DRY_RUN" == true ]]; then
        info "[dry run] Would back up the current binaries, dashboard and database to ${BACKUP_PATH}"
        return 0
    fi
    mkdir -p "${BACKUP_PATH}/bin"
    chmod 700 "$BACKUP_PATH"
    local f
    for f in tond-server tond-proxy tond update.sh; do
        [[ -f "${BIN_DIR}/${f}" ]] && cp -p "${BIN_DIR}/${f}" "${BACKUP_PATH}/bin/${f}"
    done
    cp -Rp "${INSTALL_DIR}/dashboard" "${BACKUP_PATH}/dashboard"
    for f in tond-server.service tond-proxy.service tondbuild.slice; do
        [[ -f "${SYSTEMD_DIR}/${f}" ]] && cp -p "${SYSTEMD_DIR}/${f}" "${BACKUP_PATH}/${f}"
    done
    [[ -f "${INSTALL_DIR}/VERSION" ]] && cp -p "${INSTALL_DIR}/VERSION" "${BACKUP_PATH}/VERSION"
    {
        echo "KIND=${kind}"
        echo "FROM_VERSION=${FROM_VERSION}"
        echo "TO_VERSION=${TO_VERSION}"
        echo "CREATED_AT=$(date +%s)"
        echo "SCHEMA_BEFORE=${SCHEMA_BEFORE}"
        echo "MIGRATED=unknown"
        echo "RUN_ID=${RUN_ID}"
    } > "${BACKUP_PATH}/info.env"
    info "Backed up Tond ${FROM_VERSION} to ${BACKUP_PATH}"
}

# backup_database — copy tond.db (+ its WAL) into the backup. tond-server
# must be stopped: nothing else writes the file (tond-proxy only reads it).
backup_database() {
    [[ "$DRY_RUN" == true ]] && return 0
    local f
    for f in tond.db tond.db-wal; do
        if [[ -f "${DATA_DIR}/${f}" ]]; then
            cp -p "${DATA_DIR}/${f}" "${BACKUP_PATH}/${f}"
        fi
    done
    set_info "${BACKUP_PATH}/info.env" HAS_DATABASE 1
}

# restore_database FROM_DIR — put a backed-up database back. Both services
# must be stopped. The stale -shm goes (SQLite rebuilds it).
restore_database() {
    local from="$1"
    [[ -f "${from}/tond.db" ]] || return 1
    cp -p "${from}/tond.db" "${DATA_DIR}/tond.db"
    if [[ -f "${from}/tond.db-wal" ]]; then
        cp -p "${from}/tond.db-wal" "${DATA_DIR}/tond.db-wal"
    else
        : > "${DATA_DIR}/tond.db-wal"
    fi
    rm -f "${DATA_DIR}/tond.db-shm"
    DB_RESTORED=true
}

# prune_backups — keep the KEEP_BACKUPS newest update backups.
prune_backups() {
    [[ "$DRY_RUN" == true ]] && return 0
    local old
    # Names sort by time (tond_backup_YYYYmmdd_HHMMSS[_n]).
    old="$(find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d -name 'tond_backup_*' 2>/dev/null \
        | sort -r | tail -n +$((KEEP_BACKUPS + 1)))"
    local dir
    for dir in $old; do
        rm -rf "$dir"
        info "Removed old backup ${dir}"
    done
}

# ── Installing files ──

# install_files SRC — binaries, dashboard, units and VERSION from a release
# bundle or a backup directory.
install_files() {
    local src="$1" f
    for f in tond-server tond-proxy tond; do
        replace_file "$(src_path "$src" "$f")" "${BIN_DIR}/${f}" 755 || return 1
    done
    if [[ -f "$(src_path "$src" update.sh)" ]]; then
        replace_file "$(src_path "$src" update.sh)" "${BIN_DIR}/update.sh" 755 || return 1
    fi
    mkdir -p "$(dirname "$CLI_PATH")" || return 1
    replace_file "${BIN_DIR}/tond" "$CLI_PATH" 755 || return 1

    rm -rf "${INSTALL_DIR}/dashboard.new"
    cp -R "${src}/dashboard" "${INSTALL_DIR}/dashboard.new" || return 1
    rm -rf "${INSTALL_DIR}/dashboard" || return 1
    mv "${INSTALL_DIR}/dashboard.new" "${INSTALL_DIR}/dashboard" || return 1

    for f in tond-server.service tond-proxy.service tondbuild.slice; do
        if [[ -f "${src}/${f}" ]]; then
            cp "${src}/${f}" "${SYSTEMD_DIR}/${f}" || return 1
        fi
    done
    systemctl daemon-reload || return 1

    if [[ -f "${src}/VERSION" ]]; then
        cp "${src}/VERSION" "${INSTALL_DIR}/VERSION" || return 1
    fi
}

# ── Rolling back after a failure ──

# restore_previous REASON SERVER_STARTED — put BACKUP_PATH back (the version
# that was running when this run started, FROM_VERSION) and start it again.
# Ends the script. SERVER_STARTED: whether the new tond-server was started
# (only then can the database have changed).
restore_previous() {
    local reason="$1" server_started="$2" schema_after="" what
    if [[ "$RUN_KIND" == "rollback" ]]; then
        what="The rollback to ${TO_VERSION}"
    else
        what="The update to ${TO_VERSION}"
    fi
    step rollback "Putting Tond ${FROM_VERSION} back: ${reason}"
    if [[ "$server_started" == true ]]; then
        schema_after="$(api_field schema_version)"
    fi
    systemctl stop tond-proxy 2>/dev/null || true
    systemctl stop tond-server 2>/dev/null || true
    local ok=true
    install_files "$BACKUP_PATH" || ok=false
    if [[ "$ok" == true && "$server_started" == true ]] \
        && db_restore_needed "$SCHEMA_BEFORE" "$schema_after"; then
        if restore_database "$BACKUP_PATH"; then
            info "Put the database back as it was before this run"
        else
            ok=false
        fi
    fi
    ensure_proxy_user
    secure_data_dir
    if [[ "$ok" == true ]]; then
        systemctl start tond-server || ok=false
    fi
    if [[ "$ok" == true ]] && wait_for_server "$FROM_VERSION"; then
        systemctl start tond-proxy || true
        if wait_for_proxy; then
            if [[ "$RUN_KIND" == "rollback" ]]; then
                finish failed "${what} failed (${reason}); Tond ${FROM_VERSION} is running again. Details: journalctl -u tond-server -u tond-update" 1
            fi
            finish rolled_back "${what} failed (${reason}); Tond ${FROM_VERSION} is running again. Details: journalctl -u tond-server -u tond-update" 1
        fi
    fi
    finish failed "${what} failed (${reason}) and putting ${FROM_VERSION} back failed too: Tond may be down. To recover by hand: copy ${BACKUP_PATH}/bin/* to ${BIN_DIR}/ and ${BACKUP_PATH}/tond.db to ${DATA_DIR}/, then run systemctl start tond-server tond-proxy." 3
}

# ── Update ──

SCHEMA_BEFORE=""

run_update() {
    RUN_KIND="update"
    FROM_VERSION="$(installed_version)"
    if [[ -n "$BUNDLE_DIR" && -f "${BUNDLE_DIR}/VERSION" ]]; then
        TO_VERSION="$(version_of "${BUNDLE_DIR}/VERSION")"
    elif [[ "$VERSION" != "latest" ]]; then
        TO_VERSION="$VERSION"
    fi
    preflight

    step download "Getting Tond ${TO_VERSION:-(latest)}"
    if [[ -n "$BUNDLE_DIR" ]]; then
        [[ -d "$BUNDLE_DIR" ]] || fail_unchanged "No release bundle at ${BUNDLE_DIR}."
        BUNDLE_DIR="$(cd "$BUNDLE_DIR" && pwd)"
        check_bundle "$BUNDLE_DIR"
        [[ -n "$TO_VERSION" ]] || fail_unchanged "The bundle has no VERSION file."
        info "Installing the bundle at ${BUNDLE_DIR} (Tond ${TO_VERSION})"
    else
        if [[ -z "$TO_VERSION" ]]; then
            local tag
            tag="$(latest_release_tag || true)"
            [[ -n "$tag" ]] || fail_unchanged "Could not look up the latest release on GitHub."
            TO_VERSION="${tag#v}"
        fi
        if [[ "$FROM_VERSION" == "$TO_VERSION" && "$FORCE" == false ]]; then
            finish succeeded "Tond ${FROM_VERSION} is already installed; nothing to do (use --force to reinstall)." 0
        fi
        write_status running download "Downloading Tond ${TO_VERSION}"
        download_release "$TO_VERSION"
        [[ "$DRY_RUN" == true ]] || check_bundle "$BUNDLE_DIR"
    fi
    info "Current version: ${FROM_VERSION}; installing: ${TO_VERSION}"
    if [[ "$FROM_VERSION" == "$TO_VERSION" && "$FORCE" == false ]]; then
        finish succeeded "Tond ${FROM_VERSION} is already installed; nothing to do (use --force to reinstall)." 0
    fi
    if [[ -z "$SNAPSHOT_ID" ]]; then
        warn "No snapshot was taken first. \`tond update\` and the dashboard take one before running this script."
    fi

    step backup "Backing up Tond ${FROM_VERSION}"
    SCHEMA_BEFORE="$(api_field schema_version)"
    create_backup update || fail_unchanged "Could not back up the current version to ${BACKUP_PATH}."

    if [[ "$DRY_RUN" == true ]]; then
        info "[dry run] Would stop tond-server, copy the database, install ${TO_VERSION},"
        info "[dry run] start tond-server (migrations), check its health, restart tond-proxy,"
        info "[dry run] and roll back to ${FROM_VERSION} if any of that fails."
        exit 0
    fi

    step install "Installing Tond ${TO_VERSION}"
    systemctl stop tond-server || fail_unchanged "Could not stop tond-server."
    if ! backup_database; then
        systemctl start tond-server || true
        fail_unchanged "Could not copy the database into ${BACKUP_PATH}."
    fi
    if ! install_files "$BUNDLE_DIR"; then
        restore_previous "the new files could not be installed" false
    fi
    ensure_proxy_user
    ensure_terminal_user
    secure_data_dir
    configure_journald

    step migrate "Starting Tond ${TO_VERSION} (database migrations run now)"
    if ! systemctl start tond-server; then
        restore_previous "tond-server did not start" true
    fi
    if ! wait_for_server "$TO_VERSION"; then
        restore_previous "tond-server ${TO_VERSION} did not become healthy within ${HEALTH_TIMEOUT} s" true
    fi
    local schema_after
    schema_after="$(api_field schema_version)"
    set_info "${BACKUP_PATH}/info.env" SCHEMA_AFTER "$schema_after"
    set_info "${BACKUP_PATH}/info.env" MIGRATED "$(migrated_flag "$SCHEMA_BEFORE" "$schema_after")"
    info "tond-server ${TO_VERSION} is healthy"

    step proxy "Restarting tond-proxy"
    systemctl restart tond-proxy || true
    if ! wait_for_proxy; then
        restore_previous "tond-proxy did not start" true
    fi

    prune_backups
    finish succeeded "Updated Tond ${FROM_VERSION} → ${TO_VERSION}. You can roll back for 7 days (System → Updates, or update.sh --rollback)." 0
}

# ── Rollback ──

# find_rollback_backup — the newest update backup to the installed version,
# within the rollback window (empty when there is none).
find_rollback_backup() {
    local now dir created
    now="$(date +%s)"
    find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d -name 'tond_backup_*' 2>/dev/null | sort -r \
        | while read -r dir; do
            rollback_backup_ok "$dir" "$FROM_VERSION" "$now" && { echo "$dir"; break; }
        done
    return 0
}

# rollback_backup_ok DIR CURRENT_VERSION NOW — DIR was made by the update to
# CURRENT_VERSION, less than 7 days ago, and holds the binaries and database.
rollback_backup_ok() {
    local info="$1/info.env" created
    [[ -f "$info" && -f "$1/bin/tond-server" && -f "$1/tond.db" ]] || return 1
    [[ "$(info_value "$info" KIND)" == "update" ]] || return 1
    [[ "$(info_value "$info" TO_VERSION)" == "$2" ]] || return 1
    created="$(info_value "$info" CREATED_AT)"
    [[ "$created" =~ ^[0-9]+$ ]] || return 1
    (($3 >= created && $3 - created <= ROLLBACK_WINDOW_SECS))
}

run_rollback() {
    RUN_KIND="rollback"
    FROM_VERSION="$(installed_version)"
    preflight

    step backup "Finding the version to roll back to"
    local target="$ROLLBACK_BACKUP" now
    now="$(date +%s)"
    if [[ -z "$target" ]]; then
        target="$(find_rollback_backup)"
    fi
    if [[ -z "$target" ]] || ! rollback_backup_ok "$target" "$FROM_VERSION" "$now"; then
        fail_unchanged "There is no previous version to roll back to: rollback is offered for 7 days after an update to the installed version (${FROM_VERSION})."
    fi
    local update_backup="$target"
    TO_VERSION="$(info_value "${update_backup}/info.env" FROM_VERSION)"
    local migrated
    migrated="$(info_value "${update_backup}/info.env" MIGRATED)"
    info "Rolling back Tond ${FROM_VERSION} → ${TO_VERSION} (from ${update_backup})"
    if [[ "$migrated" != "0" ]]; then
        warn "The update to ${FROM_VERSION} changed the database, so the database from before it comes back too: changes made since are lost."
    fi

    SCHEMA_BEFORE="$(api_field schema_version)"
    create_backup rollback
    if [[ "$DRY_RUN" == true ]]; then
        info "[dry run] Would stop Tond, install ${TO_VERSION} from ${update_backup}$([[ "$migrated" != "0" ]] && echo ', restore its database'), and start it."
        exit 0
    fi

    step install "Installing Tond ${TO_VERSION}"
    systemctl stop tond-proxy || true
    systemctl stop tond-server || fail_unchanged "Could not stop tond-server."
    if ! backup_database; then
        systemctl start tond-server tond-proxy || true
        fail_unchanged "Could not copy the database into ${BACKUP_PATH}."
    fi
    # From here on a failure puts back what was just backed up (the running
    # version, FROM_VERSION) through restore_previous().
    local rollback_to="$TO_VERSION"
    if ! install_files "$update_backup"; then
        restore_previous "the previous version's files could not be installed" false
    fi
    if [[ "$migrated" != "0" ]]; then
        # restore_database sets DB_RESTORED; the pre-rollback copy is in BACKUP_PATH.
        restore_database "$update_backup" || restore_previous "the database from before the update is missing" true
        info "Restored the database from before the update"
    fi
    ensure_proxy_user
    secure_data_dir

    step migrate "Starting Tond ${rollback_to}"
    if ! systemctl start tond-server || ! wait_for_server "$rollback_to"; then
        restore_previous "tond-server ${rollback_to} did not become healthy" true
    fi
    step proxy "Starting tond-proxy"
    systemctl start tond-proxy || true
    if ! wait_for_proxy; then
        restore_previous "tond-proxy did not start" true
    fi
    # The update backup is spent: rolling back twice would go nowhere.
    set_info "${update_backup}/info.env" KIND used
    finish succeeded "Rolled back Tond ${FROM_VERSION} → ${rollback_to}." 0
}

main() {
    parse_args "$@"
    STARTED_AT="$(date +%s)"
    [[ -n "$RUN_ID" ]] || RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-shell"
    [[ -n "$ACTOR" ]] || ACTOR="root (update.sh)"
    if [[ "$MODE" == "rollback" ]]; then
        run_rollback
    else
        run_update
    fi
}

trap 'echo; error "Interrupted"; write_status failed "${CURRENT_STEP:-start}" "Interrupted during the ${CURRENT_STEP:-first} step." || true; exit 130' INT TERM
# Anything that fails without its own handling: say where, then stop.
trap 'rc=$?; write_status failed "${CURRENT_STEP:-start}" "Unexpected error (exit ${rc}) at line ${LINENO} during the ${CURRENT_STEP:-first} step; see update.log." || true' ERR

# Sourced by the tests (release/tests/update-test.sh) for its functions only.
if [[ "${TOND_UPDATE_SOURCE_ONLY:-}" != "1" ]]; then
    main "$@"
fi
