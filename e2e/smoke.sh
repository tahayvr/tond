#!/usr/bin/env bash
set -euo pipefail

# Tond end-to-end smoke test.
#
# Runs as root ON a host where Tond was just installed with release/install.sh
# (GitHub Actions runner, local VM, or a throwaway Hetzner server) and drives
# the real system: systemd services, Docker, tond-proxy routing + ACME, the CLI
# over its Unix socket, and the API over TCP.
#
# Environment:
#   E2E_BASE_DOMAIN       (required) apps are served at <name>.<base>, e.g. tond.test
#                         or 203-0-113-7.sslip.io
#   E2E_DASHBOARD_DOMAIN  dashboard domain passed to install.sh (default: dash.<base>)
#   E2E_PREVIEW_DOMAIN    preview domain passed to install.sh (e.g. preview.test, a
#                         different site than the base domain). When unset the
#                         preview checks are skipped.
#   E2E_TARGET_IP         IP that the domains resolve to (default: 127.0.0.1)
#   E2E_CA_FILE           root CA of the ACME server (Pebble / LE staging). When set,
#                         the test waits for a real certificate and verifies it
#                         against this CA; when unset the TLS issuance check is skipped.
#   E2E_FIXTURE_DIR       static site pushed via `tond apps push`
#                         (default: fixtures/static-site next to this script)
#   E2E_CLEANUP           1 to delete the created resources at the end (default: 0)
#   E2E_ADMIN_EMAIL       admin account the auth checks create (via the setup token)
#   E2E_ADMIN_PASSWORD    or sign in with on re-runs (defaults below)

BASE="${E2E_BASE_DOMAIN:?E2E_BASE_DOMAIN is required}"
DASH="${E2E_DASHBOARD_DOMAIN:-dash.${BASE}}"
PREVIEW="${E2E_PREVIEW_DOMAIN:-}"
IP="${E2E_TARGET_IP:-127.0.0.1}"
CA_FILE="${E2E_CA_FILE:-}"
FIXTURE_DIR="${E2E_FIXTURE_DIR:-$(cd "$(dirname "$0")" && pwd)/fixtures/static-site}"
CLEANUP="${E2E_CLEANUP:-0}"
SOCK="/opt/tond/data/tond.sock"
DATA_DIR="/opt/tond/data"
ADMIN_EMAIL="${E2E_ADMIN_EMAIL:-e2e-admin@tond.test}"
ADMIN_PASSWORD="${E2E_ADMIN_PASSWORD:-E2e-Smoke-Passw0rd!}"

WHOAMI_DOMAIN="whoami.${BASE}"
STATIC_DOMAIN="static.${BASE}"

# ── Output helpers ────────────────────────────────────────────────────────────

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
step() { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }
pass() { echo -e "  ${GREEN}✓${NC} $*"; }
fail() { echo -e "  ${RED}✗ $*${NC}" >&2; exit 1; }

# Dump enough state to debug a failure from CI logs alone.
diagnostics() {
    local code=$?
    [[ $code -eq 0 ]] && return
    echo -e "\n${RED}${BOLD}Smoke test failed (exit ${code}). Diagnostics:${NC}"
    systemctl --no-pager status tond-server tond-proxy || true
    echo "--- tond-server (last 80 lines)"; journalctl -u tond-server --no-pager -n 80 || true
    echo "--- tond-proxy (last 80 lines)"; journalctl -u tond-proxy --no-pager -n 80 || true
    # Crash loops push the cause out of the tail above; surface it separately.
    for svc in tond-server tond-proxy; do
        echo "--- ${svc}: errors, panics and restarts"
        journalctl -u "$svc" --no-pager -o cat |
            grep -iE "panic|error|abrt|core-dump|CryptoProvider|restart counter" |
            grep -v "handshake error" | sort | uniq -c | sort -rn | head -20 || true
    done
    echo "--- containers"; docker ps -a || true
    echo "--- apps"; api /api/apps | jq . || true
}
trap diagnostics EXIT

# ── Request helpers ───────────────────────────────────────────────────────────

# GET an API path over the CLI's Unix socket (trusted, no credentials).
api() { curl -sf --unix-socket "$SOCK" "http://localhost$1"; }

# api_req <METHOD> <path> <json>: send JSON over the socket, print the HTTP
# status; the response body lands in $API_BODY.
API_BODY=$(mktemp)
api_req() {
    curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" -X "$1" \
        -H 'Content-Type: application/json' -d "$3" "http://localhost$2"
}

# api_send <method> <path> <json>: prints "<status> <body>" (does not fail on 4xx).
api_send() {
    curl -s --unix-socket "$SOCK" -X "$1" -H 'Content-Type: application/json' -d "$3" \
        -w ' %{http_code}' "http://localhost$2" | awk '{ code = $NF; $NF = ""; print code " " $0 }'
}

# "<mode> <owner> <group>" of a path.
perms() { stat -c '%a %U %G' "$1"; }

# HTTP status code of an HTTPS request to <domain><path> through tond-proxy.
# -k: before ACME finishes, the proxy serves a self-signed fallback cert.
proxied_status() {
    curl -sk -o /dev/null -w '%{http_code}' --resolve "$1:443:${IP}" "https://$1$2"
}
proxied_body() { curl -sk --resolve "$1:443:${IP}" "https://$1$2"; }

# wait_for <timeout-seconds> <description> <command...>: retry until the command succeeds.
wait_for() {
    local timeout=$1 desc=$2; shift 2
    local deadline=$((SECONDS + timeout))
    until "$@" > /dev/null 2>&1; do
        (( SECONDS < deadline )) || fail "timed out after ${timeout}s waiting for: ${desc}"
        sleep 3
    done
}

app_field() { api /api/apps | jq -r --arg n "$1" ".data[] | select(.name == \$n) | .$2"; }

# Wait until an app is running; fail fast (with deploy logs) if it fails.
wait_for_app_running() {
    local name=$1 timeout=$2 deadline=$((SECONDS + $2)) status
    while true; do
        # Tolerate transient API errors (e.g. during a restart) instead of exiting via set -e.
        status=$(app_field "$name" status || true)
        case "$status" in
            running) return 0 ;;
            failed)
                tond apps deployments "$name" || true
                tond apps logs "$name" || true
                fail "app ${name} failed to deploy"
                ;;
        esac
        (( SECONDS < deadline )) || fail "timed out after ${timeout}s waiting for ${name} (status: ${status})"
        sleep 3
    done
}

delete_e2e_resources() {
    for app in e2e-whoami e2e-static e2e-rooted e2e-compose; do
        tond apps delete "$app" -y > /dev/null 2>&1 || true
    done
    tond db delete e2e-redis -y > /dev/null 2>&1 || true
    tond cron delete e2e-cron -y > /dev/null 2>&1 || true
}

# ── Tests ─────────────────────────────────────────────────────────────────────

[[ $EUID -eq 0 ]] || fail "run as root (the CLI uses the root-only socket)"
for tool in curl jq docker tond; do command -v "$tool" > /dev/null || fail "missing tool: $tool"; done

# Re-runs on a long-lived VM start from a clean slate.
delete_e2e_resources

step "Services"
for svc in tond-server tond-proxy; do
    systemctl is-active --quiet "$svc" || fail "$svc is not running"
    pass "$svc is running"
done
wait_for 30 "API health" curl -sf http://127.0.0.1:3001/health
pass "API health check"
SECRET_ENV=/opt/tond/data/tond.env
secret_env_state="$(stat -c '%a %U' "$SECRET_ENV" 2>&1 || true)"
if [[ "$secret_env_state" != "600 root" ]] \
    || ! grep -qE '^TOND_SECRET_KEY=[0-9a-f]{64}$' "$SECRET_ENV"; then
    # Describe the file without printing the key.
    echo "    stat: ${secret_env_state}" >&2
    awk '{ i = index($0, "="); printf "    line: %s (%d chars after =)\n", substr($0, 1, i), length($0) - i }' \
        "$SECRET_ENV" >&2 || true
    fail "install.sh must generate TOND_SECRET_KEY into ${SECRET_ENV} (0600 root)"
fi
pass "encryption key generated in tond.env (0600 root)"

step "Data directory permissions (tond.db, keys and certs are not world-readable)"
[[ "$(perms "$DATA_DIR")" == "3770 root tond-proxy" ]] \
    || fail "${DATA_DIR} must be 3770 root:tond-proxy, got: $(perms "$DATA_DIR")"
pass "${DATA_DIR} is 3770 root:tond-proxy (setgid + sticky, no access for others)"
for f in tond.db tond.db-wal tond.db-shm; do
    [[ -e "${DATA_DIR}/${f}" ]] || fail "${f} must exist (persistent WAL: tond-proxy reads it and cannot create it)"
    [[ "$(perms "${DATA_DIR}/${f}")" == "640 root tond-proxy" ]] \
        || fail "${f} must be 640 root:tond-proxy, got: $(perms "${DATA_DIR}/${f}")"
done
pass "tond.db (+ -wal/-shm) is 640 root:tond-proxy (read-only for the proxy)"
world=$(find "$DATA_DIR" -maxdepth 1 -mindepth 1 -perm /o+rwx)
[[ -z "$world" ]] || fail "top-level entries accessible to others: ${world}"
for d in keys backups volumes databases builds compose docker-config; do
    [[ ! -e "${DATA_DIR}/${d}" || "$(perms "${DATA_DIR}/${d}")" == "700 root root" ]] \
        || fail "${DATA_DIR}/${d} must be 700 root:root, got: $(perms "${DATA_DIR}/${d}")"
done
pass "no top-level entry is accessible to others; private subdirectories are 0700 root"
if runuser -u nobody -- cat "${DATA_DIR}/tond.db" > /dev/null 2>&1; then
    fail "an unrelated user must not be able to read tond.db"
fi
pass "an unrelated user cannot read tond.db"

step "Service sandboxing (tond-proxy unprivileged, both units hardened)"
PROXY_PID=$(systemctl show -p MainPID --value tond-proxy)
[[ "$(ps -o user= -p "$PROXY_PID" | tr -d ' ')" == tond-proxy ]] \
    || fail "tond-proxy must run as the tond-proxy user, got: $(ps -o user= -p "$PROXY_PID")"
pass "tond-proxy runs as tond-proxy (pid ${PROXY_PID})"
# CAP_NET_BIND_SERVICE is capability 10 → 0x400; nothing else may be effective.
cap_eff=$(awk '/^CapEff:/ { print $2 }' "/proc/${PROXY_PID}/status")
[[ "$cap_eff" == 0000000000000400 ]] || fail "tond-proxy must only hold CAP_NET_BIND_SERVICE, CapEff=${cap_eff}"
pass "tond-proxy holds only CAP_NET_BIND_SERVICE"
# Root-only secrets in the shared data dir: the encryption key, the root API
# key and (until setup completes) the setup token.
for f in "$SECRET_ENV" "${DATA_DIR}/root-api-key" "${DATA_DIR}/setup-token"; do
    [[ -e "$f" ]] || continue
    [[ "$(perms "$f")" == "600 root "* ]] || fail "${f} must be 0600 root, got: $(perms "$f")"
    if runuser -u tond-proxy -- cat "$f" > /dev/null 2>&1; then
        fail "tond-proxy must not be able to read ${f}"
    fi
done
if runuser -u tond-proxy -- test -w /var/run/docker.sock; then
    fail "tond-proxy must not have access to the Docker socket"
fi
if runuser -u tond-proxy -- rm -f "$SECRET_ENV" 2> /dev/null || [[ ! -f "$SECRET_ENV" ]]; then
    fail "tond-proxy must not be able to remove root's files from the data directory"
fi
pass "tond-proxy cannot read tond.env, root-api-key or setup-token, remove root's files, or reach Docker"
# Container runtimes (OrbStack/LXC machines) may ship a drop-in that resets
# sandboxing directives; there, check the unit's configuration instead of the
# effective value. Real VMs (CI runners, Hetzner) check the effective value.
for svc in tond-server tond-proxy; do
    for prop in NoNewPrivileges=yes PrivateTmp=yes ProtectSystem=strict ProtectHome=yes \
        RestrictSUIDSGID=yes ProtectKernelModules=yes LockPersonality=yes; do
        key="${prop%%=*}" want="${prop#*=}"
        [[ "$(systemctl show -p "$key" --value "$svc")" == "$want" ]] && continue
        if systemd-detect-virt --container -q \
            && grep -qE "^${key}=(${want}|true)$" "/etc/systemd/system/${svc}.service"; then
            echo "  - ${svc}: ${key} is configured but overridden by the container runtime"
            continue
        fi
        fail "${svc}: expected ${prop}, got $(systemctl show -p "$key" "$svc")"
    done
done
[[ "$(systemctl show -p UMask --value tond-proxy)" == 0077 ]] || fail "tond-proxy must run with UMask=0077"
pass "both units: NoNewPrivileges, PrivateTmp, ProtectSystem=strict, ProtectHome, RestrictSUIDSGID, ...; proxy UMask=0077"

step "Runtime guard rails (memory, tasks, OOM, graceful stop, build slice)"
for svc in tond-server tond-proxy; do
    [[ "$(systemctl show -p OOMScoreAdjust --value "$svc")" == -900 ]] \
        || fail "${svc}: OOMScoreAdjust must be -900, got $(systemctl show -p OOMScoreAdjust --value "$svc")"
    for key in MemoryMax TasksMax; do
        value=$(systemctl show -p "$key" --value "$svc")
        [[ -n "$value" && "$value" != infinity ]] || fail "${svc}: ${key} must be set, got '${value}'"
    done
    systemctl show -p Environment --value "$svc" | tr ' ' '\n' | grep -qx MALLOC_ARENA_MAX=2 \
        || fail "${svc}: MALLOC_ARENA_MAX=2 must be set"
    pid=$(systemctl show -p MainPID --value "$svc")
    [[ "$(cat "/proc/${pid}/oom_score_adj")" == -900 ]] || fail "${svc}: oom_score_adj of pid ${pid} must be -900"
done
pass "both units: MemoryMax, TasksMax, OOMScoreAdjust=-900 (effective), MALLOC_ARENA_MAX=2"
[[ "$(systemctl show -p KillMode --value tond-server)" == mixed ]] \
    || fail "tond-server must use KillMode=mixed so SIGTERM reaches only tond-server"
pass "tond-server: KillMode=mixed (graceful shutdown handles in-flight work)"
[[ -f /etc/systemd/system/tondbuild.slice ]] || fail "install.sh must install tondbuild.slice"
systemctl is-active --quiet tondbuild.slice || fail "tondbuild.slice must be active (tond-server Wants= it)"
# The cgroup BuildKit puts RUN steps under must carry the limit itself.
slice_max=$(cat /sys/fs/cgroup/tondbuild.slice/memory.max)
[[ "$slice_max" =~ ^[0-9]+$ ]] || fail "/sys/fs/cgroup/tondbuild.slice must cap memory, got memory.max=${slice_max}"
pass "tondbuild.slice is active and caps builds at $((slice_max / 1024 / 1024)) MiB (60% of RAM)"
if [[ -d /sys/fs/cgroup/tondbuild.slice/buildkit ]]; then
    pass "BuildKit runs build steps inside tondbuild.slice"
fi

step "tond-proxy control socket and read-only database"
PROXY_SOCK="${DATA_DIR}/proxy.sock"
[[ -S "$PROXY_SOCK" ]] || fail "tond-proxy must listen on ${PROXY_SOCK}"
[[ "$(perms "$PROXY_SOCK")" == "600 tond-proxy "* ]] || fail "proxy.sock must be 0600 tond-proxy, got: $(perms "$PROXY_SOCK")"
if runuser -u nobody -- curl -s --max-time 2 --unix-socket "$PROXY_SOCK" http://x/ > /dev/null 2>&1; then
    fail "an unrelated user must not be able to connect to the proxy control socket"
fi
pass "proxy.sock is 0600 tond-proxy; other users cannot connect"
if runuser -u tond-proxy -- bash -c ": >> '${DATA_DIR}/tond.db'" 2>/dev/null; then
    fail "tond-proxy must not be able to write tond.db"
fi
pass "tond-proxy cannot write tond.db"
self=$(api /api/system/self)
echo "$self" | jq -e '.data.proxy.connected == true' > /dev/null \
    || fail "tond-server must hold a control connection to tond-proxy: $(echo "$self" | jq -c .data.proxy)"
echo "$self" | jq -e '.data.proxy.stats.route_version >= 0 and .data.proxy.stats.pid > 0' > /dev/null \
    || fail "tond-proxy must report its stats over the control socket"
pass "tond-server ↔ tond-proxy control link is up (proxy pid $(echo "$self" | jq -r .data.proxy.stats.pid))"

step "Tond measuring itself (/api/system/self, /metrics)"
echo "$self" | jq -e '.data.server.rss_bytes > 0 and .data.server.tokio_alive_tasks > 0 and .data.server.open_fds > 0' > /dev/null \
    || fail "/api/system/self must report RSS, tasks and fds: $(echo "$self" | jq -c .data.server)"
echo "$self" | jq -e '[.data.loops[].name] | index("health") and index("cron") and index("proxy-link")' > /dev/null \
    || fail "/api/system/self must list the background loops: $(echo "$self" | jq -c '[.data.loops[].name]')"
echo "$self" | jq -e '.data.db.size_bytes > 0 and .data.db.queries_total > 0' > /dev/null \
    || fail "/api/system/self must report DB size and statement counts"
echo "$self" | jq -e '.data.http | length > 0' > /dev/null || fail "/api/system/self must report HTTP latency per route"
pass "/api/system/self: RSS $(( $(echo "$self" | jq .data.server.rss_bytes) / 1024 )) KiB, $(echo "$self" | jq .data.server.tokio_alive_tasks) tasks, $(echo "$self" | jq '.data.loops | length') loops, DB $(echo "$self" | jq .data.db.size_bytes) bytes"
metrics=$(curl -sf --unix-socket "$SOCK" http://localhost/metrics)
for m in tond_process_resident_memory_bytes tond_loop_wakeups_total tond_db_queries_total tond_http_request_duration_seconds_bucket; do
    grep -q "^${m}" <<<"$metrics" || fail "/metrics must expose ${m}"
done
pass "/metrics on the root-only socket exposes Prometheus metrics"
[[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3001/metrics)" == 404 ]] \
    || fail "/metrics must not be served over TCP"
pass "/metrics over TCP → 404"

id -u tond > /dev/null 2>&1 || fail "install.sh must create the host terminal user 'tond'"
for group in docker sudo; do
    ! id -nG tond | tr ' ' '\n' | grep -qx "$group" \
        || fail "the host terminal user must not be in the ${group} group"
done
pass "host terminal user 'tond' exists and is unprivileged"

step "Authentication boundaries"
[[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3001/api/apps)" == 401 ]] \
    || fail "loopback TCP without credentials must get 401"
pass "loopback TCP without credentials → 401"
wait_for 30 "proxy serving the dashboard domain" \
    bash -c "[[ \$(curl -sk -o /dev/null -w '%{http_code}' --resolve '${DASH}:443:${IP}' 'https://${DASH}/') == 200 ]]"
pass "dashboard served through the proxy at ${DASH}"
[[ "$(proxied_status "$DASH" /api/apps)" == 401 ]] \
    || fail "unauthenticated API request through the proxy must get 401"
pass "unauthenticated API request through the proxy → 401"
[[ "$(stat -c '%a %U' "$SOCK")" == "600 root" ]] || fail "socket must be 0600 root, got: $(stat -c '%a %U' "$SOCK")"
pass "CLI socket is 0600 root"
if runuser -u nobody -- tond apps list > /dev/null 2>&1; then
    fail "a non-root user must not be able to use the CLI socket"
fi
pass "non-root user is refused by the CLI socket"

ROOT_KEY_FILE="${DATA_DIR}/root-api-key"
ROOT_KEY=""
if [[ -f "$ROOT_KEY_FILE" ]]; then
    [[ "$(stat -c '%a %U' "$ROOT_KEY_FILE")" == "600 root" ]] \
        || fail "root API key file must be 0600 root, got: $(stat -c '%a %U' "$ROOT_KEY_FILE")"
    ROOT_KEY=$(tr -d '[:space:]' < "$ROOT_KEY_FILE")
    [[ "$ROOT_KEY" =~ ^[0-9a-f]{64}$ ]] || fail "root API key file must hold a 64-hex key"
    pass "root API key stored in ${ROOT_KEY_FILE} (0600 root)"
    # Capture first: with pipefail, `journalctl | grep -q` can report SIGPIPE instead of a match.
    journal="$(journalctl -u tond-server --no-pager -o cat)"
    if grep -qF "$ROOT_KEY" <<< "$journal"; then
        fail "the root API key must never appear in the journal"
    fi
    pass "root API key is not in the journal"
fi
if [[ -n "$ROOT_KEY" ]]; then
    tond --server http://127.0.0.1:3001 --api-key "$ROOT_KEY" apps list > /dev/null \
        || fail "root API key must work over TCP"
    pass "root API key accepted over TCP"
    if tond --server http://127.0.0.1:3001 --api-key "$(printf '0%.0s' {1..64})" apps list > /dev/null 2>&1; then
        fail "a wrong API key must be rejected"
    fi
    pass "wrong API key rejected"
else
    echo "  - ${ROOT_KEY_FILE} not found (reused install?); skipping key checks"
fi

step "Auth hardening"
# Requests to the dashboard through tond-proxy, as a browser on the dashboard origin would send them.
DASH_ORIGIN="https://${DASH}"
COOKIES="$(mktemp)"
dash() { curl -sk --resolve "${DASH}:443:${IP}" -b "$COOKIES" -c "$COOKIES" "$@"; }
# dash_code <method> <path> [curl args...]: HTTP status of a dashboard request.
dash_code() {
    local method=$1 path=$2; shift 2
    dash -o /dev/null -w '%{http_code}' -X "$method" "$@" "https://${DASH}${path}"
}
json_post() { printf '{"token":"%s","email":"%s","password":"%s"}' "$1" "$2" "$3"; }

if dash "https://${DASH}/api/auth/status" | jq -e '.data | has("needs_setup")' > /dev/null; then
    fail "/api/auth/status must not tell anonymous callers whether setup is open"
fi
pass "/api/auth/status does not reveal whether setup is open"

SETUP_TOKEN_FILE="${DATA_DIR}/setup-token"
signed_in=false
if [[ -f "$SETUP_TOKEN_FILE" ]]; then
    [[ "$(stat -c '%a %U' "$SETUP_TOKEN_FILE")" == "600 root" ]] \
        || fail "setup token file must be 0600 root, got: $(stat -c '%a %U' "$SETUP_TOKEN_FILE")"
    SETUP_TOKEN=$(tr -d '[:space:]' < "$SETUP_TOKEN_FILE")
    [[ "$SETUP_TOKEN" =~ ^[0-9a-f]{64}$ ]] || fail "setup token file must hold a 64-hex token"
    journal="$(journalctl -u tond-server --no-pager -o cat)"
    if grep -qF "$SETUP_TOKEN" <<< "$journal"; then
        fail "the setup token must never appear in the journal"
    fi
    pass "setup token in ${SETUP_TOKEN_FILE} (0600 root), not in the journal"
    setup_link="$(tond setup-link)"
    grep -qF "setup?token=${SETUP_TOKEN}" <<< "$setup_link" || fail "tond setup-link must print the setup link"
    pass "tond setup-link prints the setup link"

    for token in "" "$(printf '0%.0s' {1..64})"; do
        code=$(dash_code POST /api/auth/setup -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
            -d "$(json_post "$token" "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
        [[ "$code" == 403 ]] || fail "setup without the right token must get 403, got ${code}"
    done
    pass "setup without the setup token → 403"

    headers="$(mktemp)"
    code=$(dash -D "$headers" -o /dev/null -w '%{http_code}' -X POST "https://${DASH}/api/auth/setup" \
        -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
        -d "$(json_post "$SETUP_TOKEN" "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
    [[ "$code" == 201 ]] || fail "setup with the token must create the admin (got ${code})"
    grep -iE '^set-cookie: __Host-tond_session=[0-9a-f]{64}; Secure; HttpOnly; SameSite=Strict; Path=/' "$headers" > /dev/null \
        || fail "session cookie must be __Host-tond_session with Secure; HttpOnly; SameSite=Strict; Path=/ (got: $(grep -i '^set-cookie' "$headers"))"
    rm -f "$headers"
    pass "setup with the token creates the admin and sets a __Host- Secure SameSite=Strict cookie"
    [[ ! -e "$SETUP_TOKEN_FILE" ]] || fail "the setup token file must be deleted after setup"
    # (with the CSRF header: the session cookie from setup is in the jar now)
    code=$(dash_code POST /api/auth/setup -H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' \
        -d "$(json_post "$SETUP_TOKEN" "second@tond.test" "$ADMIN_PASSWORD")")
    [[ "$code" == 403 ]] || fail "a used setup token must get 403, got ${code}"
    pass "setup token is single-use and its file is removed"
    signed_in=true
else
    code=$(dash_code POST /api/auth/login -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
        -d "$(printf '{"email":"%s","password":"%s"}' "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
    if [[ "$code" == 200 ]] && dash "https://${DASH}/api/auth/me" | jq -e '.success' > /dev/null; then
        signed_in=true
        pass "signed in as ${ADMIN_EMAIL} (setup was completed by an earlier run)"
    else
        echo "  - setup already done and ${ADMIN_EMAIL} can't sign in (got ${code}); skipping session checks"
    fi
fi

headers="$(dash -D - -o /dev/null "https://${DASH}/")"
grep -qi "^content-security-policy:.*frame-ancestors 'none'" <<< "$headers" || fail "dashboard must send a CSP with frame-ancestors 'none'"
grep -qi '^x-content-type-options: nosniff' <<< "$headers" || fail "dashboard must send X-Content-Type-Options: nosniff"
grep -qi '^referrer-policy: same-origin' <<< "$headers" || fail "dashboard must send Referrer-Policy: same-origin"
grep -qi '^strict-transport-security: max-age=' <<< "$headers" || fail "dashboard over HTTPS must send HSTS"
pass "security headers (CSP frame-ancestors, nosniff, Referrer-Policy, HSTS) on the dashboard"
direct_headers="$(curl -s -D - -o /dev/null http://127.0.0.1:3001/)"
if grep -qi '^strict-transport-security' <<< "$direct_headers"; then
    fail "HSTS must only be sent for requests that came through the TLS proxy"
fi
pass "no HSTS on plain loopback HTTP"

code=$(curl -sk --resolve "${DASH}:443:${IP}" --http1.1 -o /dev/null -w '%{http_code}' \
    -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
    -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "Origin: https://evil.${DASH}" \
    "https://${DASH}/api/system/logs/stream")
[[ "$code" == 403 ]] || fail "a WebSocket upgrade from a foreign origin must get 403, got ${code}"
pass "cross-origin WebSocket upgrade → 403"

if [[ "$signed_in" == true ]]; then
    code=$(dash_code PUT /api/settings -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 403 ]] || fail "a cookie-authenticated mutation without X-Tond-CSRF must get 403, got ${code}"
    code=$(dash_code PUT /api/settings -H "Origin: https://evil.${DASH}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 403 ]] || fail "a mutation from a foreign origin must get 403, got ${code}"
    code=$(dash_code PUT /api/settings -H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 200 ]] || fail "a same-origin mutation with X-Tond-CSRF must succeed, got ${code}"
    pass "cookie mutations need X-Tond-CSRF and the dashboard origin"

    csrf=(-H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' -H 'Content-Type: application/json')
    code=$(dash_code POST /api/auth/2fa/setup "${csrf[@]}" -d '{"password":"not-my-password"}')
    [[ "$code" == 401 ]] || fail "2FA setup with a wrong password must get 401, got ${code}"
    pass "2FA setup requires the current password"

    tond settings host-terminal off > /dev/null
    ticket_body='{"target_kind":"server","target_id":"server","password":"'"${ADMIN_PASSWORD}"'"}'
    resp=$(dash -w '\n%{http_code}' -X POST "https://${DASH}/api/terminal/tickets" "${csrf[@]}" -d "$ticket_body")
    [[ "$(tail -1 <<< "$resp")" == 403 ]] || fail "host terminal tickets must be refused while it is off"
    grep -qF 'tond settings host-terminal on' <<< "$resp" || fail "the refusal must name the enable command"
    pass "host terminal is off by default and says how to enable it"
    code=$(dash_code PUT /api/settings/host-terminal "${csrf[@]}" -d '{"enabled":true}')
    [[ "$code" == 403 ]] || fail "the dashboard must not be able to enable the host terminal, got ${code}"
    if [[ -n "$ROOT_KEY" ]]; then
        code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer ${ROOT_KEY}" \
            -H 'Content-Type: application/json' -d '{"enabled":true}' http://127.0.0.1:3001/api/settings/host-terminal)
        [[ "$code" == 403 ]] || fail "an API key must not be able to enable the host terminal, got ${code}"
    fi
    pass "only the root CLI socket can switch the host terminal"

    tond settings host-terminal on > /dev/null
    code=$(dash_code POST /api/terminal/tickets "${csrf[@]}" \
        -d '{"target_kind":"server","target_id":"server","password":"wrong-password"}')
    [[ "$code" == 401 ]] || fail "host terminal tickets must require the password (step-up), got ${code}"
    resp=$(dash -X POST "https://${DASH}/api/terminal/tickets" "${csrf[@]}" -d "$ticket_body")
    ticket=$(jq -r '.data.ticket // empty' <<< "$resp")
    [[ -n "$ticket" ]] || fail "host terminal ticket with the right password must be issued: ${resp}"
    ws_code() {
        curl -sk --resolve "${DASH}:443:${IP}" --http1.1 -o /dev/null -w '%{http_code}' --max-time 5 \
            -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
            -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "Origin: $1" \
            "https://${DASH}/api/terminal/ws?ticket=$2" || true
    }
    # curl can't speak WebSocket here; after a 101 it just waits until --max-time.
    code=$(ws_code "$DASH_ORIGIN" "$ticket")
    [[ "$code" != 401 && "$code" != 403 ]] || fail "a fresh ticket must open the terminal WebSocket, got ${code}"
    code=$(ws_code "$DASH_ORIGIN" "$ticket")
    [[ "$code" == 401 ]] || fail "a terminal ticket must be single-use, got ${code}"
    pass "host terminal: step-up password, single-use ticket"
    if [[ -n "$ROOT_KEY" ]]; then
        code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer ${ROOT_KEY}" \
            -H 'Content-Type: application/json' -d "$ticket_body" http://127.0.0.1:3001/api/terminal/tickets)
        [[ "$code" == 403 ]] || fail "API keys must not get terminal tickets, got ${code}"
        pass "API keys are refused terminal tickets"
    fi
    tond settings host-terminal off > /dev/null
    audit="$(api '/api/audit-log?limit=20')"
    grep -q 'terminal_opened' <<< "$audit" || fail "terminal sessions must be audited"
    pass "terminal sessions are written to the audit log"
fi

# Rotating X-Forwarded-For must not create fresh rate-limit buckets: the proxy
# replaces it with the real peer. 200 requests, 25 at a time (fast enough to
# beat the 10/s refill even on a slow or emulated host), exceed the 60-request
# burst.
limited=false
codes=$(seq 1 200 | xargs -P 25 -I{} curl -sk --resolve "${DASH}:443:${IP}" -o /dev/null \
    -w '%{http_code}\n' -H "X-Forwarded-For: 198.51.100.{}" "https://${DASH}/api/auth/status")
[[ "$codes" == *429* ]] && limited=true
[[ "$limited" == true ]] || fail "a spoofed X-Forwarded-For must not bypass the rate limiter"
pass "spoofed X-Forwarded-For does not bypass the rate limiter"
sleep 7 # let the bucket refill

# Failed logins back off per client (unknown emails, rotating X-Forwarded-For).
codes=""
for i in $(seq 1 6); do
    codes+="$(curl -sk --resolve "${DASH}:443:${IP}" -o /dev/null -w '%{http_code} ' \
        -X POST -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
        -H "X-Forwarded-For: 203.0.113.${i}" \
        -d "{\"email\":\"nobody${i}@tond.test\",\"password\":\"wrong\"}" "https://${DASH}/api/auth/login")"
done
[[ "$codes" == *429* ]] || fail "repeated failed logins must be throttled (got: ${codes})"
pass "failed logins back off (got: ${codes% })"
rm -f "$COOKIES"

step "Image app (pull + run + route)"
tond apps create e2e-whoami --source-type image --source traefik/whoami:v1.10 \
    --port 80 --domain "$WHOAMI_DOMAIN" > /dev/null
tond apps deploy e2e-whoami > /dev/null
wait_for_app_running e2e-whoami 300
pass "e2e-whoami is running"
wait_for 60 "route for ${WHOAMI_DOMAIN}" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
pass "https://${WHOAMI_DOMAIN} is routed to the container"
[[ "$(curl -s -o /dev/null -w '%{http_code}' --resolve "${WHOAMI_DOMAIN}:80:${IP}" "http://${WHOAMI_DOMAIN}/")" == 301 ]] \
    || fail "plain HTTP should redirect to HTTPS"
pass "HTTP redirects to HTTPS"
[[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: not-served.example.net' "http://${IP}/")" == 404 ]] \
    || fail "plain HTTP for a host Tond does not serve must be 404, not an open redirect"
pass "HTTP for an unknown host → 404 (no open redirect)"

step "Webhook and integration hardening"
API_TCP="http://127.0.0.1:3001"
# status_of <curl args...>: HTTP status code of a request to the TCP API (no credentials).
status_of() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
WHOAMI_ID=$(app_field e2e-whoami id)
[[ "$WHOAMI_ID" =~ ^[0-9a-f-]{36}$ ]] || fail "could not read the e2e-whoami app id (got: ${WHOAMI_ID})"
WEBHOOK_JSON=$(api "/api/apps/${WHOAMI_ID}/webhook")
WEBHOOK_SECRET=$(jq -r '.data.secret // empty' <<< "$WEBHOOK_JSON")
[[ ${#WEBHOOK_SECRET} -ge 32 ]] || fail "every app must get a generated webhook secret"
pass "app has a generated webhook secret"
[[ "$(jq -r '.data.paths.github' <<< "$WEBHOOK_JSON")" == "/api/webhooks/github/${WHOAMI_ID}" ]] \
    || fail "webhook URLs must be addressed by app UUID"
pass "webhook URLs are addressed by app UUID"

PUSH_BODY='{"ref":"refs/heads/main","after":"e2e"}'
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -H 'Content-Type: application/json' \
    -d "$PUSH_BODY" "${API_TCP}/api/webhooks/github/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unsigned GitHub webhook must be rejected"
pass "unsigned GitHub webhook → 401"
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -H 'X-Hub-Signature-256: sha256=deadbeef' \
    -d "$PUSH_BODY" "${API_TCP}/api/webhooks/github/${WHOAMI_ID}")" == 401 ]] \
    || fail "a wrongly signed GitHub webhook must be rejected"
pass "wrongly signed GitHub webhook → 401"
[[ "$(status_of -X POST -H 'X-Gitlab-Event: Push Hook' -H 'X-Gitlab-Token: wrong' \
    -d "$PUSH_BODY" "${API_TCP}/api/webhooks/gitlab/${WHOAMI_ID}")" == 401 ]] \
    || fail "a GitLab webhook with a wrong token must be rejected"
pass "GitLab webhook with a wrong token → 401"
[[ "$(status_of -X POST -H 'X-Gitea-Event: push' \
    -d "$PUSH_BODY" "${API_TCP}/api/webhooks/gitea/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unsigned Gitea webhook must be rejected"
pass "unsigned Gitea webhook → 401"
[[ "$(status_of -X POST "${API_TCP}/api/webhooks/generic/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unauthenticated generic webhook must be rejected"
pass "unauthenticated generic webhook → 401"
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -d "$PUSH_BODY" \
    "${API_TCP}/api/webhooks/github/e2e-whoami")" == 404 ]] \
    || fail "webhooks must not resolve apps by name"
pass "webhook by app name → 404"

# Authenticated generic call: accepted (auto-deploy is off, so nothing deploys),
# and replaying the same delivery ID is refused.
DELIVERY="e2e-$(date +%s)-$$"
generic_signed() {
    status_of -X POST -H "Authorization: Bearer ${WEBHOOK_SECRET}" -H "X-Tond-Delivery: ${DELIVERY}" \
        "${API_TCP}/api/webhooks/generic/${WHOAMI_ID}"
}
[[ "$(generic_signed)" == 200 ]] || fail "a generic webhook with the app secret must be accepted"
pass "generic webhook with the app secret → 200"
[[ "$(generic_signed)" == 409 ]] || fail "a replayed webhook delivery must be rejected"
pass "replayed delivery → 409"

if command -v openssl > /dev/null; then
    PING_BODY="{\"zen\":\"e2e ${DELIVERY}\"}"
    SIG=$(printf '%s' "$PING_BODY" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | awk '{print $NF}')
    [[ "$(status_of -X POST -H 'X-GitHub-Event: ping' -H "X-GitHub-Delivery: ${DELIVERY}-gh" \
        -H "X-Hub-Signature-256: sha256=${SIG}" -d "$PING_BODY" \
        "${API_TCP}/api/webhooks/github/${WHOAMI_ID}")" == 200 ]] \
        || fail "a correctly signed GitHub webhook must be accepted"
    pass "correctly signed GitHub webhook → 200"
else
    echo "  - openssl not installed; skipping the signed GitHub webhook check"
fi

[[ "$(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" -X PUT \
    -H 'Content-Type: application/json' -d '{"secret":null}' \
    "http://localhost/api/apps/${WHOAMI_ID}/webhook-secret")" != 200 ]] \
    || fail "the webhook secret must not be clearable"
pass "webhook secret cannot be cleared"
ROTATED=$(curl -sf --unix-socket "$SOCK" -X POST \
    "http://localhost/api/apps/${WHOAMI_ID}/webhook-secret/rotate" | jq -r '.data.secret // empty')
[[ ${#ROTATED} -ge 32 && "$ROTATED" != "$WEBHOOK_SECRET" ]] || fail "rotating the webhook secret must return a new one"
[[ "$(status_of -X POST -H "Authorization: Bearer ${WEBHOOK_SECRET}" \
    "${API_TCP}/api/webhooks/generic/${WHOAMI_ID}")" == 401 ]] \
    || fail "the old webhook secret must stop working after rotation"
pass "rotated secret replaces the old one"

GH_APPS_BEFORE=$(api /api/github-apps | jq '.data | length')
# GitHub's redirect only hands off to the dashboard; completing needs the
# admin's session (same-origin POST) and their pending, single-use state.
callback=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' \
    "${API_TCP}/api/github-apps/callback?code=e2eattackercode&state=forged")
[[ "$callback" == "303 "*"/integrations/github/complete?kind=manifest"* ]] \
    || fail "the GitHub App callback must hand off to the dashboard (got: ${callback})"
completion=$(curl -s -X POST -H 'Content-Type: application/json' \
    -d '{"kind":"manifest","code":"e2eattackercode"}' "${API_TCP}/api/github-apps/complete")
[[ "$(jq -r .error <<< "$completion")" == missing_state ]] \
    || fail "completing a GitHub App flow without state must be refused (got: ${completion})"
completion=$(curl -s -X POST -H 'Content-Type: application/json' \
    -d '{"kind":"manifest","code":"e2eattackercode","state":"forged"}' "${API_TCP}/api/github-apps/complete")
[[ "$(jq -r .error <<< "$completion")" == not_logged_in ]] \
    || fail "completing a GitHub App flow without an admin session must be refused (got: ${completion})"
[[ "$(api /api/github-apps | jq '.data | length')" == "$GH_APPS_BEFORE" ]] \
    || fail "a refused GitHub App callback must not store an App"
pass "GitHub App callback without the admin's pending state is refused"
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -d "$PUSH_BODY" \
    "${API_TCP}/webhooks/source/github/events")" == 401 ]] \
    || fail "unsigned GitHub App events must be rejected"
pass "unsigned GitHub App event → 401"

step "Built app (tond apps push → TBuild → docker build)"
tond apps create e2e-static --source-type tarball --port 8080 --domain "$STATIC_DOMAIN" > /dev/null
(cd "$FIXTURE_DIR" && tond apps push e2e-static > /dev/null)
wait_for_app_running e2e-static 600
wait_for 60 "route for ${STATIC_DOMAIN}" bash -c "curl -sk --resolve '${STATIC_DOMAIN}:443:${IP}' 'https://${STATIC_DOMAIN}/' | grep -q tond-e2e-static-ok"
pass "pushed static site is built, running and routed"

step "TLS certificate issuance (ACME HTTP-01)"
if [[ -n "$CA_FILE" ]]; then
    wait_for 240 "a certificate for ${WHOAMI_DOMAIN} signed by ${CA_FILE}" \
        curl -sf --cacert "$CA_FILE" --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/"
    pass "${WHOAMI_DOMAIN} serves a certificate that verifies against the ACME CA"
    wait_for 30 "certificate state for ${WHOAMI_DOMAIN} recorded as active" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/certs | jq -e '.data[] | select(.domain == \"${WHOAMI_DOMAIN}\" and .status == \"active\" and .failure_count == 0 and .next_retry_at == null and .last_error == null)'"
    pass "certificate state (status, retry, last error) recorded for the dashboard"
else
    echo "  - E2E_CA_FILE not set; skipping certificate verification"
fi
# Issued (or self-signed fallback) certificates and the ACME account belong to
# tond-proxy and are owner-only.
bad=$(find "${DATA_DIR}/certs" \( -type f ! -perm 600 \) -o \( -type d ! -perm 700 \) -o ! -user tond-proxy)
[[ -z "$bad" ]] || fail "certs must be tond-proxy-owned 0600 files / 0700 dirs: ${bad}"
if [[ -e "${DATA_DIR}/acme_account.json" ]]; then
    [[ "$(stat -c '%a %U' "${DATA_DIR}/acme_account.json")" == "600 tond-proxy" ]] \
        || fail "acme_account.json must be 600 tond-proxy, got: $(perms "${DATA_DIR}/acme_account.json")"
fi
pass "TLS keys and the ACME account key are 0600, owned by tond-proxy"

step "Preview domain isolation"
if [[ -n "$PREVIEW" ]]; then
    # Capture first: with pipefail, `tond settings | grep -q` can fail on SIGPIPE.
    settings_out="$(tond settings)"
    grep -qE "^preview-domain +${PREVIEW}$" <<< "$settings_out" \
        || fail "install.sh must set the preview domain to ${PREVIEW}"
    pass "preview domain ${PREVIEW} configured by install.sh"

    read -r code body < <(api_send PUT /api/settings "{\"preview_domain\": \"previews.${BASE}\"}")
    [[ "$code" == 400 ]] || fail "a preview domain on the dashboard's site must be rejected (got ${code}: ${body})"
    pass "a preview domain on the dashboard's site (previews.${BASE}) is rejected"

    read -r code body < <(api_send POST /api/apps/e2e-whoami/preview '{"ttl_hours": 1}')
    [[ "$code" == 202 ]] || fail "creating a preview must succeed (got ${code}: ${body})"
    PREVIEW_ID=$(jq -r .data.id <<< "$body")
    PREVIEW_HOST=$(jq -r .data.subdomain <<< "$body")
    [[ "$PREVIEW_HOST" == *"-e2e-whoami.${PREVIEW}" ]] || fail "preview host must be under ${PREVIEW}, got: ${PREVIEW_HOST}"
    wait_for 180 "preview ${PREVIEW_HOST} running" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/previews | jq -e '.data[] | select(.id == \"${PREVIEW_ID}\" and .status == \"running\")'"
    wait_for 60 "route for ${PREVIEW_HOST}" \
        bash -c "curl -sk --resolve '${PREVIEW_HOST}:443:${IP}' 'https://${PREVIEW_HOST}/' | grep -q Hostname:"
    pass "preview served at https://${PREVIEW_HOST}"

    for host in "$PREVIEW" "dash.${PREVIEW}" "unknown.${PREVIEW}"; do
        status=$(proxied_status "$host" /api/apps)
        [[ "$status" == 404 ]] || fail "https://${host}/api/apps must be 404 (dashboard/API never on the preview domain), got ${status}"
    done
    pass "the dashboard/API is never served on the preview domain (404)"
    PREVIEW_ON_DASH="${PREVIEW_HOST%%."${PREVIEW}"}.${DASH}"
    if curl -sk --resolve "${PREVIEW_ON_DASH}:443:${IP}" "https://${PREVIEW_ON_DASH}/" | grep -q Hostname:; then
        fail "a preview must not be served under the dashboard domain (${PREVIEW_ON_DASH})"
    fi
    pass "the preview is not served under the dashboard domain"

    if [[ -n "$CA_FILE" ]]; then
        wait_for 240 "a certificate for ${PREVIEW_HOST} signed by ${CA_FILE}" \
            curl -sf --cacert "$CA_FILE" --resolve "${PREVIEW_HOST}:443:${IP}" "https://${PREVIEW_HOST}/"
        pass "${PREVIEW_HOST} serves a certificate that verifies against the ACME CA"
    fi

    read -r code body < <(api_send PUT /api/settings '{"preview_domain": ""}')
    [[ "$code" == 200 ]] || fail "clearing the preview domain must succeed (got ${code}: ${body})"
    read -r code body < <(api_send POST /api/apps/e2e-whoami/preview '{}')
    [[ "$code" == 400 && "$body" == *"preview domain"* ]] \
        || fail "previews must be refused without a preview domain (got ${code}: ${body})"
    read -r code body < <(api_send PUT /api/settings "{\"preview_domain\": \"${PREVIEW}\"}")
    [[ "$code" == 200 ]] || fail "restoring the preview domain must succeed (got ${code}: ${body})"
    pass "previews are refused with an explanation when no preview domain is set"

    curl -sf --unix-socket "$SOCK" -X DELETE "http://localhost/api/apps/e2e-whoami/previews/${PREVIEW_ID}" > /dev/null \
        || fail "deleting the preview must succeed"
    pass "preview deleted"
else
    echo "  - E2E_PREVIEW_DOMAIN not set; skipping preview checks"
fi

step "Managed database"
tond db create e2e-redis --db-type redis > /dev/null
wait_for 240 "e2e-redis running" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/databases | jq -e '.data[] | select(.name == \"e2e-redis\" and .status == \"running\")'"
pass "e2e-redis is running"
REDIS_DETAIL=$(api /api/databases/e2e-redis)
REDIS_USER=$(jq -r '.data.db_user' <<<"$REDIS_DETAIL")
REDIS_PASS=$(jq -r '.data.db_password' <<<"$REDIS_DETAIL")
# The server may still be loading right after "running"; retry the auth check briefly.
wait_for 30 "e2e-redis answering authenticated PING" \
    bash -c "docker exec -e REDISCLI_AUTH='${REDIS_PASS}' tond-db-e2e-redis redis-cli --no-auth-warning --user '${REDIS_USER}' PING | grep -qx PONG"
noauth=$(docker exec tond-db-e2e-redis redis-cli PING 2>&1 || true)
[[ "$noauth" == *NOAUTH* ]] || fail "managed Redis must refuse unauthenticated clients, got: ${noauth}"
wrong=$(docker exec -e REDISCLI_AUTH=wrong tond-db-e2e-redis redis-cli --no-auth-warning --user "$REDIS_USER" PING 2>&1 || true)
[[ "$wrong" != *PONG* ]] || fail "managed Redis must refuse a wrong password"
jq -e --arg u "$REDIS_USER" --arg p "$REDIS_PASS" '.data.connection_string | contains($u + ":" + $p + "@")' <<<"$REDIS_DETAIL" > /dev/null \
    || fail "the Redis connection string must carry the credentials"
pass "managed Redis requires auth (NOAUTH without, PONG with the ACL user); connection string carries credentials"

step "Cron job"
tond cron create e2e-cron --schedule "0 0 1 1 *" --image alpine:3 --command "echo tond-e2e-cron-ok" > /dev/null
CRON_ID=$(api /api/cron | jq -r '.data[] | select(.name == "e2e-cron") | .id')
tond cron trigger e2e-cron > /dev/null
wait_for 180 "a successful e2e-cron run" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/cron/${CRON_ID}/runs' | jq -e '.data[] | select(.status == \"success\" and (.output // \"\" | contains(\"tond-e2e-cron-ok\")))'"
pass "manual cron run succeeded with the expected output"

step "Container confinement"
# Every container Tond starts: ports on 127.0.0.1 only (Docker's iptables rules
# bypass UFW), no-new-privileges, trimmed capabilities, pids + memory limits,
# rotated json-file logs.
assert_hardened() { # <container> <label>
    local hc
    hc=$(docker inspect "$1" | jq '.[0].HostConfig')
    jq -e '[(.PortBindings // {})[][]?.HostIp] | length > 0 and all(. == "127.0.0.1")' <<<"$hc" > /dev/null \
        || fail "$2: published ports must bind 127.0.0.1, got $(jq -c '.PortBindings' <<<"$hc")"
    jq -e '(.SecurityOpt // []) | any(.[]; . == "no-new-privileges:true")' <<<"$hc" > /dev/null \
        || fail "$2: missing no-new-privileges, got $(jq -c '.SecurityOpt' <<<"$hc")"
    jq -e '(.CapDrop // []) | any(.[]; . == "NET_RAW")' <<<"$hc" > /dev/null \
        || fail "$2: NET_RAW must be dropped, got $(jq -c '.CapDrop' <<<"$hc")"
    jq -e '(.PidsLimit // 0) > 0 and (.Memory // 0) > 0' <<<"$hc" > /dev/null \
        || fail "$2: pids and memory limits must be set, got pids=$(jq '.PidsLimit' <<<"$hc") memory=$(jq '.Memory' <<<"$hc")"
    jq -e '.LogConfig.Type == "json-file" and .LogConfig.Config["max-size"] == "10m" and .LogConfig.Config["max-file"] == "3"' <<<"$hc" > /dev/null \
        || fail "$2: logs must rotate, got $(jq -c '.LogConfig' <<<"$hc")"
    pass "$2: loopback ports, no-new-privileges, NET_RAW dropped, limits, log rotation"
}
assert_hardened tond-e2e-whoami "app e2e-whoami"
assert_hardened "tond-db-e2e-redis" "database e2e-redis"

WHOAMI_PORT=$(app_field e2e-whoami host_port)
REDIS_PORT=$(api /api/databases | jq -r '.data[] | select(.name == "e2e-redis") | .host_port')
for port in "$WHOAMI_PORT" "$REDIS_PORT"; do
    if ss -Hltn "sport = :${port}" | awk '{print $4}' | grep -vqE '^127\.0\.0\.1:'; then
        ss -ltn "sport = :${port}" >&2
        fail "port ${port} is listening on a public interface"
    fi
done
pass "published ports ${WHOAMI_PORT} and ${REDIS_PORT} only listen on 127.0.0.1"
[[ "$(api /api/databases | jq -r '.data[] | select(.name == "e2e-redis") | .public_access')" == false ]] \
    || fail "managed databases must be private by default"
pass "managed databases are private by default"

jq -e '.["log-opts"]["max-size"] and .["log-opts"]["max-file"]' /etc/docker/daemon.json > /dev/null \
    || fail "install.sh must set default log rotation in /etc/docker/daemon.json"
pass "daemon.json sets default container log rotation"

code=$(api_req POST /api/apps '{"name":"e2e-rooted","source_type":"image","source_url":"alpine:3","container_port":80,"volumes":[{"container_path":"/host","host_path":"/"}]}')
[[ "$code" == 400 ]] && grep -q "is not allowed" "$API_BODY" \
    || fail "a host_path of / must be rejected with 400, got ${code}: $(cat "$API_BODY")"
[[ -z "$(app_field e2e-rooted id)" ]] || fail "no app may be created for a rejected volume"
pass "POST /api/apps with host_path \"/\" → 400, nothing created"
code=$(api_req POST /api/apps/e2e-whoami/volumes '{"container_path":"/sock","host_path":"/var/run/docker.sock"}')
[[ "$code" == 400 ]] || fail "binding docker.sock into an app must be rejected, got ${code}: $(cat "$API_BODY")"
pass "POST /api/apps/{id}/volumes with docker.sock → 400"
code=$(api_req POST /api/apps '{"name":"e2e-compose","source_type":"compose","container_port":80,"compose_content":"services:\n  web:\n    image: nginx:alpine\n    privileged: true\n    network_mode: host\n"}')
[[ "$code" == 400 ]] && grep -q "privileged" "$API_BODY" && grep -q "network_mode: host" "$API_BODY" \
    || fail "a privileged compose file must be rejected with every problem listed, got ${code}: $(cat "$API_BODY")"
pass "compose with privileged + network_mode: host → 400 listing both problems"

step "Per-deployment images and rollback"
deployments() { api /api/apps/e2e-whoami/deployments; }
FIRST_ID=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].id')
FIRST_TAG=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].image_tag')
[[ "$FIRST_TAG" =~ ^tond-e2e-whoami:[0-9a-f]{12}$ ]] || fail "deployments must run a per-deployment tag, got ${FIRST_TAG}"
docker image inspect "$FIRST_TAG" > /dev/null || fail "image ${FIRST_TAG} must exist on the host"
pass "deployment runs ${FIRST_TAG}"
tond apps deploy e2e-whoami > /dev/null
wait_for 300 "a second successful e2e-whoami deployment" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e '[.data[] | select(.status == \"succeeded\")] | length >= 2'"
wait_for_app_running e2e-whoami 120
SECOND_TAG=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].image_tag')
[[ "$SECOND_TAG" != "$FIRST_TAG" ]] || fail "each deployment must get its own tag"
deployments | jq -e --arg id "$FIRST_ID" '.data[] | select(.id == $id) | .rollback_available' > /dev/null \
    || fail "the previous deployment must be offered for rollback"
pass "redeploy ran ${SECOND_TAG}; ${FIRST_TAG} is offered for rollback"
code=$(api_req POST /api/apps/e2e-whoami/rollback "{\"deployment_id\":\"${FIRST_ID}\"}")
[[ "$code" == 202 ]] || fail "rollback should be accepted, got ${code}: $(cat "$API_BODY")"
wait_for 300 "rollback to ${FIRST_TAG}" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e --arg t '${FIRST_TAG}' '.data[0] | select(.trigger_source == \"rollback\" and .status == \"succeeded\" and .image_tag == \$t)'"
wait_for_app_running e2e-whoami 120
running_image=$(docker inspect tond-e2e-whoami | jq -r '.[0].Config.Image')
[[ "$running_image" == "$FIRST_TAG" ]] || fail "rollback must run exactly ${FIRST_TAG}, got ${running_image}"
wait_for 60 "route after rollback" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
pass "rollback restored the exact previous image and traffic follows"

step "Build queue: coalescing, 202 + queue position, acknowledged cutover"
wait_for_app_running e2e-whoami 120
# Three deploys back to back: the first runs, the second waits, the third
# supersedes the second (one deploy in flight per app, newest wins).
q1=$(api_req POST /api/apps/e2e-whoami/deploy '{}'); b1=$(cat "$API_BODY")
q2=$(api_req POST /api/apps/e2e-whoami/deploy '{}'); b2=$(cat "$API_BODY")
q3=$(api_req POST /api/apps/e2e-whoami/deploy '{}'); b3=$(cat "$API_BODY")
[[ "$q1 $q2 $q3" == "202 202 202" ]] || fail "deploys must be accepted with 202, got ${q1} ${q2} ${q3}"
for b in "$b1" "$b2" "$b3"; do
    echo "$b" | jq -e '.data.deployment_id and .data.status == "queued" and .data.queue_position >= 1' > /dev/null \
        || fail "a queued deploy must return its deployment id and queue position: ${b}"
done
D2=$(echo "$b2" | jq -r .data.deployment_id)
D3=$(echo "$b3" | jq -r .data.deployment_id)
[[ "$(echo "$b3" | jq -r .data.superseded)" == "$D2" ]] || fail "the third deploy must supersede the second: ${b3}"
wait_for 300 "the newest deploy to succeed" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e --arg id '${D3}' '.data[] | select(.id == \$id and .status == \"succeeded\")'"
api /api/apps/e2e-whoami/deployments | jq -e --arg id "$D2" --arg by "$D3" \
    '.data[] | select(.id == $id and .status == "superseded" and .superseded_by == $by)' > /dev/null \
    || fail "the waiting deploy must be marked superseded by the newest one"
pass "3 deploys → first ran, second superseded by the third, third succeeded"
wait_for_app_running e2e-whoami 120
tond apps logs e2e-whoami 2>/dev/null | grep -q "Proxy confirmed the new route in" \
    || { tond apps logs e2e-whoami | tail -20 >&2; fail "the cutover must wait for tond-proxy's acknowledgement"; }
pass "cutover waited for the proxy's acknowledgement ($(tond apps logs e2e-whoami | grep -o 'confirmed the new route in [0-9]* ms' | tail -1))"
wait_for 30 "route after queued deploys" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"

step "Snapshot"
tond snapshot create --name e2e-smoke > /dev/null
wait_for 120 "e2e-smoke snapshot completed" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/snapshots | jq -e '.data[] | select(.name == \"e2e-smoke\" and .status == \"completed\")'"
pass "full snapshot completed"

step "Restart survival"
systemctl restart tond-server tond-proxy
wait_for 30 "API health after restart" curl -sf http://127.0.0.1:3001/health
[[ "$(app_field e2e-whoami status)" == running ]] || fail "e2e-whoami should still be running after a restart"
wait_for 60 "route after restart" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
pass "apps keep running and routing after restarting tond-server and tond-proxy"
journalctl -u tond-server --since "-2min" --no-pager -o cat | grep -q "SIGTERM received: shutting down gracefully" \
    || fail "tond-server must shut down gracefully on SIGTERM"
pass "tond-server shut down gracefully on SIGTERM"
# A restart in the middle of a deploy: the deployment is marked interrupted,
# never left 'building'/'deploying'.
api_req POST /api/apps/e2e-whoami/deploy '{}' > /dev/null
INTERRUPTED=$(jq -r .data.deployment_id "$API_BODY")
systemctl kill -s SIGKILL tond-server
wait_for 60 "API health after a crash" curl -sf http://127.0.0.1:3001/health
status=$(api /api/apps/e2e-whoami/deployments | jq -r --arg id "$INTERRUPTED" '.data[] | select(.id == $id) | .status')
case "$status" in
    failed)
        api /api/apps/e2e-whoami/deployments | jq -e --arg id "$INTERRUPTED" '.data[] | select(.id == $id) | .error | test("Interrupted")' > /dev/null \
            || fail "an interrupted deploy must say so"
        pass "a deploy cut off by a crash is marked failed (interrupted) at startup" ;;
    queued|building|deploying|succeeded)
        # It had not started yet (resumed from the queue) or finished first.
        wait_for 300 "the resumed deploy to finish" \
            bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e --arg id '${INTERRUPTED}' '.data[] | select(.id == \$id and (.status == \"succeeded\" or .status == \"failed\"))'"
        pass "a deploy caught by the crash was resumed or had finished (status was ${status})" ;;
    *) fail "unexpected status for the interrupted deploy: '${status}'" ;;
esac
unfinished=$(api /api/apps/e2e-whoami/deployments | jq '[.data[] | select(.status == "building" or .status == "deploying")] | length')
wait_for 300 "no deployment left in flight" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e '[.data[] | select(.status == \"building\" or .status == \"deploying\" or .status == \"queued\")] | length == 0'"
wait_for_app_running e2e-whoami 120
pass "startup reconciliation left nothing in flight (had ${unfinished} right after the crash)"

step "Stability"
running_runs=$(api "/api/cron/${CRON_ID}/runs" | jq '[.data[] | select(.status == "running")] | length')
[[ "$running_runs" == 0 ]] || fail "no cron run may stay 'running' after a restart (found ${running_runs})"
pass "no cron run left 'running' after the restart"
[[ "$(api "/api/cron/${CRON_ID}" | jq -r '.data.status')" != running ]] \
    || fail "the cron job must not stay 'running' after its run finished"
pass "cron job status settled after its run"

# The proxy must not log per request or per unchanged route refresh: 200
# requests (half to an unknown host) and a few 2 s refreshes add ~nothing.
since=$(date '+%Y-%m-%d %H:%M:%S')
for _ in $(seq 1 100); do
    curl -sk -o /dev/null --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/" || true
    curl -sk -o /dev/null --resolve "unknown.${BASE}:443:${IP}" "https://unknown.${BASE}/" || true
done
sleep 6
proxy_lines=$(journalctl -u tond-proxy --since "$since" --no-pager -o cat | wc -l)
if (( proxy_lines > 20 )); then
    journalctl -u tond-proxy --since "$since" --no-pager -o cat | sort | uniq -c | sort -rn | head -10 >&2
    fail "tond-proxy wrote ${proxy_lines} journal lines for 200 requests (expected ≤ 20)"
fi
pass "tond-proxy journal stays quiet under traffic (${proxy_lines} lines for 200 requests)"

if [[ "$CLEANUP" == 1 ]]; then
    step "Cleanup"
    delete_e2e_resources
    pass "e2e resources deleted"
fi

echo -e "\n${GREEN}${BOLD}All smoke tests passed.${NC}"
