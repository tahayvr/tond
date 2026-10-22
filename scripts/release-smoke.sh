#!/usr/bin/env bash
set -euo pipefail

# Real-release smoke test: install a PUBLISHED Tond release on a fresh server
# the way users do, check the signature path, then update to a second
# published release and roll back.
#
#   1. install --from with the tond.io installer (release/web/install.sh from
#      this checkout, or the live https://tond.io/install), downloading from
#      the public GitHub releases or TOND_RELEASE_BASE; the output must show
#      the checksum and minisign signature verified with the embedded key
#   2. the installed version everywhere: /opt/tond/VERSION, `tond --version`,
#      GET /api/v1/system/update
#   3. the signature path against the published files: the release block of
#      the installed update.sh verifies the real .minisig, and refuses a
#      tampered tarball and a missing signature
#   4. a canary app (traefik/whoami) is deployed
#   5. with --to: `tond update --version <to>` (snapshot, tond-update.service,
#      verified download, migration, health check) → version <to>, canary up
#   6. `tond update --rollback` → version <from> again, canary up
#
# Run it from your machine against a fresh, disposable Ubuntu/Debian server
# (a Hetzner VM, an OrbStack machine, ...) that you can SSH into as root, or
# on the server itself with --local. It changes the server (installs Tond):
# never point it at a server you care about.
#
# Usage:
#   scripts/release-smoke.sh --host root@203.0.113.7 --from 0.2.0 [--to 0.2.1]
#                            [--release-base <url>] [--installer repo|tond.io]
#                            [--install-args "<install.sh args>"] [--ssh-opts "<opts>"]
#                            [--allow-existing]
#   scripts/release-smoke.sh --local --from 0.2.0 [--to 0.2.1] ...   (as root on the server)
#
#   --release-base  where releases come from (default: TOND_RELEASE_BASE, else
#                   https://github.com/tahayvr/tond/releases); a local mirror
#                   needs <base>/download/<tag>/… and <base>/latest
#   --installer     repo (default): this checkout's release/web/install.sh;
#                   tond.io: curl https://tond.io/install (what users run)
#   --install-args  extra install.sh arguments (default: --non-interactive;
#                   the dashboard defaults to <ip>.sslip.io on a public IP)
#   --allow-existing  don't insist on a server without /opt/tond

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST=""
LOCAL=false
REMOTE=false
FROM=""
TO=""
BASE="${TOND_RELEASE_BASE:-}"
INSTALLER="repo"
INSTALL_ARGS="--non-interactive"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"
ALLOW_EXISTING=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --host) HOST="$2"; shift 2 ;;
        --local) LOCAL=true; shift ;;
        --remote) REMOTE=true; LOCAL=true; shift ;; # internal: running on the server
        --from) FROM="${2#v}"; shift 2 ;;
        --to) TO="${2#v}"; shift 2 ;;
        --release-base) BASE="$2"; shift 2 ;;
        --installer) INSTALLER="$2"; shift 2 ;;
        --install-args) INSTALL_ARGS="$2"; shift 2 ;;
        --ssh-opts) SSH_OPTS="$2"; shift 2 ;;
        --allow-existing) ALLOW_EXISTING=true; shift ;;
        -h|--help) sed -n '4,42p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ -n "$FROM" ]] || { echo "--from <version> is required (see --help)" >&2; exit 2; }
[[ "$INSTALLER" == repo || "$INSTALLER" == tond.io ]] || { echo "--installer must be repo or tond.io" >&2; exit 2; }

# ── From your machine: ship this script, e2e/lib.sh and the installer ─────────

if [[ "$LOCAL" == false ]]; then
    [[ -n "$HOST" ]] || { echo "--host user@server (or --local on the server) is required" >&2; exit 2; }
    # shellcheck disable=SC2206
    ssh_opts=($SSH_OPTS)
    work="/root/tond-release-smoke"
    echo "Copying the test to ${HOST}:${work}"
    ssh "${ssh_opts[@]}" "$HOST" "rm -rf ${work} && mkdir -p ${work}"
    scp -q "${ssh_opts[@]}" "$0" "${ROOT}/e2e/lib.sh" "${ROOT}/release/web/install.sh" "${HOST}:${work}/"
    args=(--remote --from "$FROM" --installer "$INSTALLER" --install-args "$INSTALL_ARGS")
    [[ -n "$TO" ]] && args+=(--to "$TO")
    [[ -n "$BASE" ]] && args+=(--release-base "$BASE")
    [[ "$ALLOW_EXISTING" == true ]] && args+=(--allow-existing)
    quoted="$(printf '%q ' "${args[@]}")"
    exec ssh "${ssh_opts[@]}" "$HOST" "bash ${work}/release-smoke.sh ${quoted}"
fi

# ── On the server ─────────────────────────────────────────────────────────────

HERE="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "${HERE}/lib.sh" ]]; then LIB="${HERE}/lib.sh"; else LIB="${ROOT}/e2e/lib.sh"; fi
# shellcheck source=../e2e/lib.sh
. "$LIB"
if [[ -f "${HERE}/install.sh" && "$REMOTE" == true ]]; then
    WEB_INSTALLER="${HERE}/install.sh"
else
    WEB_INSTALLER="${ROOT}/release/web/install.sh"
fi

[[ $EUID -eq 0 ]] || fail "run as root on the server (or use --host from your machine)"
[[ "$(uname -s)" == Linux ]] || fail "run this on the Linux server (or use --host from your machine)"
for tool in curl jq tar; do
    command -v "$tool" > /dev/null || { apt-get update -qq && apt-get install -y -qq curl jq tar > /dev/null; break; }
done
if [[ -d /opt/tond && "$ALLOW_EXISTING" == false ]]; then
    fail "/opt/tond exists: use a fresh server (or --allow-existing)"
fi

LOGS="$(mktemp -d)"
[[ -n "$BASE" ]] && export TOND_RELEASE_BASE="$BASE"
SHOWN_BASE="${BASE:-https://github.com/tahayvr/tond/releases}"
ARCH="$(uname -m)"
case "$ARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; esac

on_exit() {
    local code=$?
    if [[ $code -ne 0 ]]; then
        echo -e "\n${RED}${BOLD}Release smoke failed (exit ${code}). Logs in ${LOGS}${NC}"
        systemctl --no-pager status tond-server tond-proxy 2> /dev/null | head -30 || true
        journalctl -u tond-server -u tond-update --no-pager -n 60 2> /dev/null || true
    fi
}
trap on_exit EXIT

# The installed version, three ways.
assert_version() { # version label
    local file cli api_v
    file="$(tr -d 'v[:space:]' < /opt/tond/VERSION)"
    cli="$(tond --version | awk '{print $2}' | tr -d v)"
    api_v="$(current_version)"
    [[ "$file" == "$1" && "$cli" == "$1" && "$api_v" == "$1" ]] \
        || fail "$2: expected ${1} everywhere, got VERSION=${file}, tond --version=${cli}, API=${api_v}"
    pass "$2: Tond ${1} (VERSION, tond --version and the API agree)"
}

step "Install Tond ${FROM} from ${SHOWN_BASE} (installer: ${INSTALLER})"
if [[ "$INSTALLER" == tond.io ]]; then
    curl -fsSL https://tond.io/install -o "${LOGS}/web-install.sh"
    WEB_INSTALLER="${LOGS}/web-install.sh"
fi
# shellcheck disable=SC2086
if ! TOND_VERSION="$FROM" sh "$WEB_INSTALLER" $INSTALL_ARGS 2>&1 | tee "${LOGS}/install.log"; then
    fail "the install failed (${LOGS}/install.log)"
fi
grep -q "Checksum verified" "${LOGS}/install.log" || fail "the installer must verify the checksum"
grep -q "Signature of v${FROM} verified" "${LOGS}/install.log" \
    || fail "the installer must verify the minisign signature of v${FROM} with its embedded key"
pass "downloaded v${FROM} (tond-linux-${ARCH}), checksum and signature verified"
wait_for 60 "API health" curl -sf http://127.0.0.1:3001/health
assert_version "$FROM" "after install"
if [[ -n "$BASE" && "${BASE%/}" != "https://github.com/tahayvr/tond/releases" ]]; then
    [[ "$(cat /opt/tond/RELEASE_BASE)" == "${BASE%/}" ]] || fail "/opt/tond/RELEASE_BASE must record ${BASE}"
    pass "the release base is recorded for updates (/opt/tond/RELEASE_BASE)"
else
    [[ ! -e /opt/tond/RELEASE_BASE ]] || fail "no RELEASE_BASE file for the default base"
fi

step "Signature path against the published v${FROM} files"
block="${LOGS}/release-block.sh"
sed -n '/^# ── Release signatures (keep in sync/,/^# ── end release signatures ──/p' /opt/tond/bin/update.sh > "$block"
grep -q '^fetch_verified_release()' "$block" || fail "the installed update.sh has no release block"
dl="${LOGS}/dl" && mkdir -p "$dl"
bash -c ". '$block'; fetch_verified_release 'v${FROM}' 'tond-linux-${ARCH}' '$dl'" > "${LOGS}/verify.log" 2>&1 \
    || fail "the published v${FROM} must verify: $(cat "${LOGS}/verify.log")"
cp "${dl}/tond.tar.gz" "${dl}/tampered.tar.gz" && printf 'x' >> "${dl}/tampered.tar.gz"
if bash -c ". '$block'; check_release_signature 'v${FROM}' '${dl}/tampered.tar.gz' '${dl}/tond.tar.gz.minisig'" > /dev/null 2>&1; then
    fail "a tampered tarball must not verify against the published signature"
fi
if env -u TOND_ALLOW_UNSIGNED bash -c ". '$block'; check_release_signature 'v${FROM}' '${dl}/tond.tar.gz' '${dl}/none.minisig'" > /dev/null 2>&1; then
    fail "a release without a signature must be refused"
fi
pass "the published signature verifies; a tampered tarball and a missing signature are refused"

step "Canary app"
tond apps create release-canary --source-type image --source traefik/whoami:v1.10 --port 80 > /dev/null
tond apps deploy release-canary > /dev/null
wait_for_app_running release-canary 240
pass "release-canary is running on ${FROM}"

if [[ -n "$TO" ]]; then
    step "Update ${FROM} → ${TO} (tond update: snapshot, verified download, migrate, health check)"
    if ! tond update --version "$TO" 2>&1 | tee "${LOGS}/update.log"; then
        fail "tond update --version ${TO} failed (${LOGS}/update.log)"
    fi
    jq -e '.state == "succeeded" and .kind == "update"' /opt/tond/data/update/status.json > /dev/null \
        || fail "the update must end succeeded: $(cat /opt/tond/data/update/status.json)"
    grep -q "Signature of v${TO} verified" /opt/tond/data/update/update.log \
        || fail "the update must verify the signature of v${TO} (update.log)"
    wait_for 60 "API health after the update" curl -sf http://127.0.0.1:3001/health
    assert_version "$TO" "after the update"
    api /api/v1/snapshots | jq -e '.data[] | select(.trigger == "pre_update" and .status == "completed")' > /dev/null \
        || fail "the update must take a pre-update snapshot"
    wait_for_app_running release-canary 120
    pass "update to ${TO}: signature verified, pre-update snapshot, canary still running"

    step "Roll back ${TO} → ${FROM} (tond update --rollback)"
    if ! tond update --rollback 2>&1 | tee "${LOGS}/rollback.log"; then
        fail "tond update --rollback failed (${LOGS}/rollback.log)"
    fi
    jq -e '.state == "succeeded" and .kind == "rollback"' /opt/tond/data/update/status.json > /dev/null \
        || fail "the rollback must end succeeded: $(cat /opt/tond/data/update/status.json)"
    wait_for 60 "API health after the rollback" curl -sf http://127.0.0.1:3001/health
    assert_version "$FROM" "after the rollback"
    wait_for_app_running release-canary 120
    pass "rollback to ${FROM}: canary still running"
else
    echo "  - no --to: skipping the update and rollback"
fi

tond apps delete release-canary -y > /dev/null 2>&1 || true
echo -e "\n${GREEN}${BOLD}Release smoke passed${NC} (v${FROM}${TO:+ → v${TO} → v${FROM}}, tond-linux-${ARCH}, ${SHOWN_BASE})"
