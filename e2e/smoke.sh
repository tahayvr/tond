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
#   E2E_APPS_DOMAIN       apps domain passed to install.sh (e.g. apps.test). When set,
#                         an app without a custom domain must be served at
#                         <app>.<apps-domain>; when unset that check is skipped.
#   E2E_TARGET_IP         IP that the domains resolve to (default: 127.0.0.1)
#   E2E_CA_FILE           root CA of the ACME server (Pebble / LE staging). When set,
#                         the test waits for a real certificate and verifies it
#                         against this CA; when unset the TLS issuance check is skipped.
#   E2E_CHALLTESTSRV      management URL of Pebble's challtestsrv (default
#                         http://127.0.0.1:8055). With E2E_CA_FILE and E2E_APPS_DOMAIN,
#                         the domains step orders a DNS-01 wildcard through it
#                         (tond-server must run with TOND_DNS_TEST_PROVIDER=1, which
#                         e2e/run.sh sets for --acme pebble; the installer never does).
#   E2E_FIXTURE_DIR       static site pushed via `tond apps push`
#                         (default: fixtures/static-site next to this script)
#   E2E_CLEANUP           1 to delete the created resources at the end (default: 0)
#   E2E_ADMIN_EMAIL       admin account the auth checks create (via the setup token)
#   E2E_ADMIN_PASSWORD    or sign in with on re-runs (defaults below)
#   E2E_BUNDLE            the installed release bundle (set by run.sh); enables the
#                         update.sh install/rollback checks of the self-update step

BASE="${E2E_BASE_DOMAIN:?E2E_BASE_DOMAIN is required}"
DASH="${E2E_DASHBOARD_DOMAIN:-dash.${BASE}}"
PREVIEW="${E2E_PREVIEW_DOMAIN:-}"
APPS_DOMAIN="${E2E_APPS_DOMAIN:-}"
IP="${E2E_TARGET_IP:-127.0.0.1}"
CA_FILE="${E2E_CA_FILE:-}"
CHALLTESTSRV="${E2E_CHALLTESTSRV:-http://127.0.0.1:8055}"
FIXTURE_DIR="${E2E_FIXTURE_DIR:-$(cd "$(dirname "$0")" && pwd)/fixtures/static-site}"
CLEANUP="${E2E_CLEANUP:-0}"
SOCK="/opt/tond/data/tond.sock"
DATA_DIR="/opt/tond/data"
ADMIN_EMAIL="${E2E_ADMIN_EMAIL:-e2e-admin@tond.test}"
ADMIN_PASSWORD="${E2E_ADMIN_PASSWORD:-E2e-Smoke-Passw0rd!}"

WHOAMI_DOMAIN="whoami.${BASE}"
STATIC_DOMAIN="static.${BASE}"
VAULT_DOMAIN="vault.${BASE}"

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
    echo "--- apps"; api /api/v1/apps | jq . || true
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
        -H 'Content-Type: application/json' -d "${3:-}" "http://localhost$2"
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

# watch_availability <domain> <file>: request https://<domain>/ every 0.2 s
# until killed, one HTTP status per line (000 when nothing answered).
watch_availability() {
    while true; do
        curl -sk -o /dev/null -w '%{http_code}\n' --max-time 5 \
            --resolve "$1:443:${IP}" "https://$1/" >> "$2" 2> /dev/null || true
        sleep 0.2
    done
}

# assert_no_downtime <file> <label>: every recorded status was 2xx.
assert_no_downtime() {
    local total bad
    total=$(wc -l < "$1" | tr -d ' ')
    bad=$(grep -cv '^2' "$1" || true)
    (( total >= 5 )) || fail "$2: only ${total} requests were made during the redeploy"
    (( bad == 0 )) || fail "$2: ${bad} of ${total} requests during the redeploy were not 2xx: $(sort "$1" | uniq -c | tr '\n' ' ')"
    pass "$2: ${total} requests during the redeploy, all 2xx"
}

app_field() { api /api/v1/apps | jq -r --arg n "$1" ".data[] | select(.name == \$n) | .$2"; }

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
    for app in e2e-whoami e2e-static e2e-rooted e2e-compose e2e-stack e2e-badport e2e-worker e2e-worker-domain e2e-auto e2e-wild e2e-vault; do
        tond apps delete "$app" -y > /dev/null 2>&1 || true
    done
    # A template refuses a name whose volume directory still holds data.
    rm -rf "${DATA_DIR}/volumes/e2e-vault"
    for db in e2e-redis e2e-redis-copy e2e-pg e2e-pg-copy; do
        tond db delete "$db" -y > /dev/null 2>&1 || true
    done
    tond cron delete e2e-cron -y > /dev/null 2>&1 || true
    tond projects delete e2e-envproj -y > /dev/null 2>&1 || true
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
# Root-only secrets in the shared data dir: the encryption key and (until
# setup completes) the setup token. There is no root API key any more.
[[ ! -e "${DATA_DIR}/root-api-key" ]] || fail "no root API key file may be written (use tond token create)"
for f in "$SECRET_ENV" "${DATA_DIR}/setup-token"; do
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
pass "tond-proxy cannot read tond.env or setup-token, remove root's files, or reach Docker"
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
# The link reconnects with backoff after install starts the proxy; give it a moment.
wait_for 30 "tond-server holding a control connection to tond-proxy" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/system/self | jq -e '.data.proxy.connected == true'"
self=$(api /api/v1/system/self)
echo "$self" | jq -e '.data.proxy.stats.route_version >= 0 and .data.proxy.stats.pid > 0' > /dev/null \
    || fail "tond-proxy must report its stats over the control socket"
pass "tond-server ↔ tond-proxy control link is up (proxy pid $(echo "$self" | jq -r .data.proxy.stats.pid))"

step "Tond measuring itself (/api/v1/system/self, /metrics)"
echo "$self" | jq -e '.data.server.rss_bytes > 0 and .data.server.tokio_alive_tasks > 0 and .data.server.open_fds > 0' > /dev/null \
    || fail "/api/v1/system/self must report RSS, tasks and fds: $(echo "$self" | jq -c .data.server)"
echo "$self" | jq -e '[.data.loops[].name] | index("health") and index("cron") and index("proxy-link")' > /dev/null \
    || fail "/api/v1/system/self must list the background loops: $(echo "$self" | jq -c '[.data.loops[].name]')"
echo "$self" | jq -e '.data.db.size_bytes > 0 and .data.db.queries_total > 0' > /dev/null \
    || fail "/api/v1/system/self must report DB size and statement counts"
echo "$self" | jq -e '.data.http | length > 0' > /dev/null || fail "/api/v1/system/self must report HTTP latency per route"
pass "/api/v1/system/self: RSS $(( $(echo "$self" | jq .data.server.rss_bytes) / 1024 )) KiB, $(echo "$self" | jq .data.server.tokio_alive_tasks) tasks, $(echo "$self" | jq '.data.loops | length') loops, DB $(echo "$self" | jq .data.db.size_bytes) bytes"
metrics=$(curl -sf --unix-socket "$SOCK" http://localhost/metrics)
for m in tond_process_resident_memory_bytes tond_loop_wakeups_total tond_db_queries_total tond_http_request_duration_seconds_bucket; do
    grep -q "^${m}" <<<"$metrics" || fail "/metrics must expose ${m}"
done
pass "/metrics on the root-only socket exposes Prometheus metrics"
[[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3001/metrics)" == 401 ]] \
    || fail "/metrics over TCP must require a metrics:read token"
pass "/metrics over TCP without a token → 401"

id -u tond > /dev/null 2>&1 || fail "install.sh must create the host terminal user 'tond'"
for group in docker sudo; do
    ! id -nG tond | tr ' ' '\n' | grep -qx "$group" \
        || fail "the host terminal user must not be in the ${group} group"
done
pass "host terminal user 'tond' exists and is unprivileged"

step "Authentication boundaries"
[[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3001/api/v1/apps)" == 401 ]] \
    || fail "loopback TCP without credentials must get 401"
pass "loopback TCP without credentials → 401"
wait_for 30 "proxy serving the dashboard domain" \
    bash -c "[[ \$(curl -sk -o /dev/null -w '%{http_code}' --resolve '${DASH}:443:${IP}' 'https://${DASH}/') == 200 ]]"
pass "dashboard served through the proxy at ${DASH}"
[[ "$(proxied_status "$DASH" /api/v1/apps)" == 401 ]] \
    || fail "unauthenticated API request through the proxy must get 401"
pass "unauthenticated API request through the proxy → 401"
[[ "$(stat -c '%a %U' "$SOCK")" == "600 root" ]] || fail "socket must be 0600 root, got: $(stat -c '%a %U' "$SOCK")"
pass "CLI socket is 0600 root"
if runuser -u nobody -- tond apps list > /dev/null 2>&1; then
    fail "a non-root user must not be able to use the CLI socket"
fi
pass "non-root user is refused by the CLI socket"

# Automation gets scoped tokens from the root CLI socket; the secret is printed once.
for name in e2e-readonly e2e-admin e2e-ci e2e-env e2e-revoked e2e-expired e2e-ip-ok e2e-ip-other; do
    tond token revoke "$name" -y > /dev/null 2>&1 || true # re-runs on a long-lived VM
done
RO_TOKEN=$(tond token create --name e2e-readonly --preset readonly --expires 1d 2> /dev/null)
[[ "$RO_TOKEN" =~ ^tond_pat_[0-9a-f]{64}$ ]] || fail "tond token create must print a tond_pat_ token, got: ${RO_TOKEN}"
pass "tond token create prints a scoped tond_pat_ token once"
journal="$(journalctl -u tond-server --no-pager -o cat)"
if grep -qF "$RO_TOKEN" <<< "$journal"; then
    fail "API tokens must never appear in the journal"
fi
pass "the token is not in the journal"
tond --server http://127.0.0.1:3001 --token "$RO_TOKEN" apps list > /dev/null \
    || fail "a read-only token must list apps over TCP"
pass "read-only token accepted over TCP"
if tond --server http://127.0.0.1:3001 --token "tond_pat_$(printf '0%.0s' {1..64})" apps list > /dev/null 2>&1; then
    fail "a wrong token must be rejected"
fi
pass "wrong token rejected"
# An admin-scoped token, used below to prove tokens can't reach root-only switches.
ADMIN_TOKEN=$(tond token create --name e2e-admin --scope settings:write --scope terminal:host --expires 1d 2> /dev/null)

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

if dash "https://${DASH}/api/v1/auth/status" | jq -e '.data | has("needs_setup")' > /dev/null; then
    fail "/api/v1/auth/status must not tell anonymous callers whether setup is open"
fi
pass "/api/v1/auth/status does not reveal whether setup is open"

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
        code=$(dash_code POST /api/v1/auth/setup -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
            -d "$(json_post "$token" "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
        [[ "$code" == 403 ]] || fail "setup without the right token must get 403, got ${code}"
    done
    pass "setup without the setup token → 403"

    headers="$(mktemp)"
    code=$(dash -D "$headers" -o /dev/null -w '%{http_code}' -X POST "https://${DASH}/api/v1/auth/setup" \
        -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
        -d "$(json_post "$SETUP_TOKEN" "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
    [[ "$code" == 201 ]] || fail "setup with the token must create the admin (got ${code})"
    grep -iE '^set-cookie: __Host-tond_session=[0-9a-f]{64}; Secure; HttpOnly; SameSite=Strict; Path=/' "$headers" > /dev/null \
        || fail "session cookie must be __Host-tond_session with Secure; HttpOnly; SameSite=Strict; Path=/ (got: $(grep -i '^set-cookie' "$headers"))"
    rm -f "$headers"
    pass "setup with the token creates the admin and sets a __Host- Secure SameSite=Strict cookie"
    [[ ! -e "$SETUP_TOKEN_FILE" ]] || fail "the setup token file must be deleted after setup"
    # (with the CSRF header: the session cookie from setup is in the jar now)
    code=$(dash_code POST /api/v1/auth/setup -H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' \
        -d "$(json_post "$SETUP_TOKEN" "second@tond.test" "$ADMIN_PASSWORD")")
    [[ "$code" == 403 ]] || fail "a used setup token must get 403, got ${code}"
    pass "setup token is single-use and its file is removed"
    signed_in=true
else
    code=$(dash_code POST /api/v1/auth/login -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' \
        -d "$(printf '{"email":"%s","password":"%s"}' "$ADMIN_EMAIL" "$ADMIN_PASSWORD")")
    if [[ "$code" == 200 ]] && dash "https://${DASH}/api/v1/auth/me" | jq -e '.success' > /dev/null; then
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
    "https://${DASH}/api/v1/system/logs/stream")
[[ "$code" == 403 ]] || fail "a WebSocket upgrade from a foreign origin must get 403, got ${code}"
pass "cross-origin WebSocket upgrade → 403"

if [[ "$signed_in" == true ]]; then
    code=$(dash_code PUT /api/v1/settings -H "Origin: ${DASH_ORIGIN}" -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 403 ]] || fail "a cookie-authenticated mutation without X-Tond-CSRF must get 403, got ${code}"
    code=$(dash_code PUT /api/v1/settings -H "Origin: https://evil.${DASH}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 403 ]] || fail "a mutation from a foreign origin must get 403, got ${code}"
    code=$(dash_code PUT /api/v1/settings -H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' \
        -H 'Content-Type: application/json' -d '{}')
    [[ "$code" == 200 ]] || fail "a same-origin mutation with X-Tond-CSRF must succeed, got ${code}"
    pass "cookie mutations need X-Tond-CSRF and the dashboard origin"

    csrf=(-H "Origin: ${DASH_ORIGIN}" -H 'X-Tond-CSRF: 1' -H 'Content-Type: application/json')
    code=$(dash_code POST /api/v1/auth/2fa/setup "${csrf[@]}" -d '{"password":"not-my-password"}')
    [[ "$code" == 401 ]] || fail "2FA setup with a wrong password must get 401, got ${code}"
    pass "2FA setup requires the current password"

    tond settings host-terminal off > /dev/null
    ticket_body='{"target_kind":"server","target_id":"server","password":"'"${ADMIN_PASSWORD}"'"}'
    resp=$(dash -w '\n%{http_code}' -X POST "https://${DASH}/api/v1/terminal/tickets" "${csrf[@]}" -d "$ticket_body")
    [[ "$(tail -1 <<< "$resp")" == 403 ]] || fail "host terminal tickets must be refused while it is off"
    grep -qF 'tond settings host-terminal on' <<< "$resp" || fail "the refusal must name the enable command"
    pass "host terminal is off by default and says how to enable it"
    code=$(dash_code PUT /api/v1/settings/host-terminal "${csrf[@]}" -d '{"enabled":true}')
    [[ "$code" == 403 ]] || fail "the dashboard must not be able to enable the host terminal, got ${code}"
    code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' -d '{"enabled":true}' http://127.0.0.1:3001/api/v1/settings/host-terminal)
    [[ "$code" == 403 ]] || fail "an API token must not be able to enable the host terminal, got ${code}"
    pass "only the root CLI socket can switch the host terminal"

    tond settings host-terminal on > /dev/null
    code=$(dash_code POST /api/v1/terminal/tickets "${csrf[@]}" \
        -d '{"target_kind":"server","target_id":"server","password":"wrong-password"}')
    [[ "$code" == 401 ]] || fail "host terminal tickets must require the password (step-up), got ${code}"
    resp=$(dash -X POST "https://${DASH}/api/v1/terminal/tickets" "${csrf[@]}" -d "$ticket_body")
    ticket=$(jq -r '.data.ticket // empty' <<< "$resp")
    [[ -n "$ticket" ]] || fail "host terminal ticket with the right password must be issued: ${resp}"
    ws_code() {
        curl -sk --resolve "${DASH}:443:${IP}" --http1.1 -o /dev/null -w '%{http_code}' --max-time 5 \
            -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
            -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "Origin: $1" \
            "https://${DASH}/api/v1/terminal/ws?ticket=$2" || true
    }
    # curl can't speak WebSocket here; after a 101 it just waits until --max-time.
    code=$(ws_code "$DASH_ORIGIN" "$ticket")
    [[ "$code" != 401 && "$code" != 403 ]] || fail "a fresh ticket must open the terminal WebSocket, got ${code}"
    code=$(ws_code "$DASH_ORIGIN" "$ticket")
    [[ "$code" == 401 ]] || fail "a terminal ticket must be single-use, got ${code}"
    pass "host terminal: step-up password, single-use ticket"
    code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' -d "$ticket_body" http://127.0.0.1:3001/api/v1/terminal/tickets)
    [[ "$code" == 403 ]] || fail "API tokens must not get terminal tickets, even with terminal:host, got ${code}"
    pass "API tokens are refused terminal tickets"
    tond settings host-terminal off > /dev/null
    audit="$(api '/api/v1/audit-log?limit=20')"
    grep -q 'terminal_opened' <<< "$audit" || fail "terminal sessions must be audited"
    pass "terminal sessions are written to the audit log"
fi

# Rotating X-Forwarded-For must not create fresh rate-limit buckets: the proxy
# replaces it with the real peer. 200 requests, 25 at a time (fast enough to
# beat the 10/s refill even on a slow or emulated host), exceed the 60-request
# burst.
limited=false
codes=$(seq 1 200 | xargs -P 25 -I{} curl -sk --resolve "${DASH}:443:${IP}" -o /dev/null \
    -w '%{http_code}\n' -H "X-Forwarded-For: 198.51.100.{}" "https://${DASH}/api/v1/auth/status")
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
        -d "{\"email\":\"nobody${i}@tond.test\",\"password\":\"wrong\"}" "https://${DASH}/api/v1/auth/login")"
done
[[ "$codes" == *429* ]] || fail "repeated failed logins must be throttled (got: ${codes})"
pass "failed logins back off (got: ${codes% })"
rm -f "$COOKIES"

step "Users, roles and the password policy"
# Invite over the root socket (users:admin), set the password through the
# one-time link, act as a viewer, then disable and delete the account.
VIEWER_EMAIL="e2e-viewer@tond.test"
old_id=$(api /api/v1/users | jq -r --arg e "$VIEWER_EMAIL" '.data[] | select(.email == $e) | .id')
[[ -z "$old_id" ]] || api_req DELETE "/api/v1/users/${old_id}" '{}' > /dev/null
code=$(api_req POST /api/v1/users "{\"email\":\"${VIEWER_EMAIL}\",\"role\":\"viewer\"}")
[[ "$code" == 201 ]] || fail "inviting a user must succeed, got ${code}: $(cat "$API_BODY")"
VIEWER_ID=$(jq -r '.data.user.id' "$API_BODY")
LINK_TOKEN=$(jq -r '.data.token' "$API_BODY")
[[ "$(jq -r '.data.user.password_set' "$API_BODY")" == false ]] || fail "invited users have no password until they use the link"
pass "invite creates a passwordless user and a one-time set-password link"
VIEWER_COOKIES="$(mktemp)"
link_complete() {
    curl -sk --resolve "${DASH}:443:${IP}" -b "$VIEWER_COOKIES" -c "$VIEWER_COOKIES" -o "$API_BODY" -w '%{http_code}' \
        -X POST -H "Origin: https://${DASH}" -H 'X-Tond-CSRF: 1' -H 'Content-Type: application/json' \
        -d "{\"token\":\"${LINK_TOKEN}\",\"password\":\"$1\"}" "https://${DASH}/api/v1/auth/password-link/complete"
}
code=$(link_complete 'Password1234!')
[[ "$code" == 400 ]] && grep -q breached "$API_BODY" || fail "a breached password must be refused with a clear message, got ${code}: $(cat "$API_BODY")"
code=$(link_complete 'short')
[[ "$code" == 400 ]] && grep -q '12 characters' "$API_BODY" || fail "passwords under 12 characters must be refused, got ${code}"
pass "password policy: 12+ characters, breached passwords refused with a friendly message"
code=$(link_complete 'violet otter lamp 42')
[[ "$code" == 200 && "$(jq -r '.data.signed_in' "$API_BODY")" == true ]] || fail "a good password must be set and sign in, got ${code}: $(cat "$API_BODY")"
[[ "$(link_complete 'another good passphrase')" == 403 ]] || fail "set-password links must be single-use"
pass "the link sets the password once and signs the user in"
viewer_code() {
    curl -sk --resolve "${DASH}:443:${IP}" -b "$VIEWER_COOKIES" -o "$API_BODY" -w '%{http_code}' \
        -X "$1" -H "Origin: https://${DASH}" -H 'X-Tond-CSRF: 1' -H 'Content-Type: application/json' \
        ${3:+-d "$3"} "https://${DASH}$2"
}
[[ "$(viewer_code GET /api/v1/apps)" == 200 ]] || fail "viewers can read apps"
code=$(viewer_code PUT /api/v1/settings '{"server_name":"viewer-was-here"}')
[[ "$code" == 403 && "$(jq -r '.error.code' "$API_BODY")" == SCOPE_DENIED ]] || fail "viewers must get 403 SCOPE_DENIED for settings, got ${code}"
[[ "$(viewer_code GET /api/v1/users)" == 403 ]] || fail "viewers can't list users"
pass "a viewer can read but gets 403 SCOPE_DENIED for writes"
code=$(api_req PATCH "/api/v1/users/${VIEWER_ID}" '{"disabled":true}')
[[ "$code" == 200 ]] || fail "disabling a user must succeed, got ${code}"
[[ "$(viewer_code GET /api/v1/apps)" == 401 ]] || fail "disabling a user must end their sessions"
pass "disabling a user signs them out everywhere"
[[ "$(api_req DELETE "/api/v1/users/${VIEWER_ID}" '{}')" == 200 ]] || fail "deleting a user must succeed"
api "/api/v1/audit-log?resource_type=user&resource_id=${VIEWER_ID}" \
    | jq -e '[.data.entries[].action] | (index("user_invite") and index("user_disable") and index("user_delete") and index("invite_accepted"))' > /dev/null \
    || fail "user lifecycle events must be audited"
pass "invite, password set, disable and delete are audited"
rm -f "$VIEWER_COOKIES"

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
if [[ -s /proc/net/if_inet6 ]]; then
    # tond-proxy listens on IPv6 too when the host has it (IPV6_V6ONLY sockets).
    ss -Htln | grep -q '\[::\]:443 ' || fail "tond-proxy must listen on [::]:443 on a host with IPv6: $(ss -Htln)"
    ss -Htln | grep -q '\[::\]:80 ' || fail "tond-proxy must listen on [::]:80 on a host with IPv6: $(ss -Htln)"
    wait_for 30 "https://${WHOAMI_DOMAIN} over IPv6" \
        bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:[::1]' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
    [[ "$(curl -s -o /dev/null -w '%{http_code}' --resolve "${WHOAMI_DOMAIN}:80:[::1]" "http://${WHOAMI_DOMAIN}/")" == 301 ]] \
        || fail "plain HTTP over IPv6 should redirect to HTTPS"
    pass "tond-proxy serves ports 80 and 443 over IPv6 as well"
else
    echo "  (no IPv6 on this host: skipping the IPv6 listener check)"
fi

step "Request metrics and access logs (tond-proxy counters, opt-in access log)"
# The proxy counts every request per route; tond-server scrapes the counters
# on its 30 s metrics tick (the not-yet-written minute is included).
REQ_N=20
for _ in $(seq 1 "$REQ_N"); do
    curl -sk -o /dev/null --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/"
done
# whoami's /health answers with the status last POSTed to it: make it a 404.
curl -sk -o /dev/null -X POST -d 404 --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/health"
code=$(proxied_status "$WHOAMI_DOMAIN" /health)
curl -sk -o /dev/null -X POST -d 200 --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/health"
[[ "$code" == 404 ]] || fail "whoami's /health should answer 404 after POSTing 404, got ${code}"
[[ "$(proxied_status "nothing-here.${BASE}" /)" == 502 ]] || fail "an unknown host must get the 502 page"
wait_for 90 "request metrics for e2e-whoami" bash -c "curl -sf --unix-socket '$SOCK' \
    'http://localhost/api/v1/apps/e2e-whoami/requests?range=1h' \
    | jq -e '.data.summary.requests >= ${REQ_N} and .data.summary.s2xx >= ${REQ_N} and .data.summary.s4xx >= 1'"
pass "after ${REQ_N} requests the API reports ≥ ${REQ_N} requests, their 2xx and the 4xx"
REQ=$(api "/api/v1/apps/e2e-whoami/requests?range=1h")
jq -e '.data.series.t | length > 0 and length <= 481' <<< "$REQ" > /dev/null \
    || fail "the request series must be server-sized, got: $(jq -c '.data.series.t | length' <<< "$REQ")"
jq -e '.data.summary.p95 != null and .data.proxy_connected == true' <<< "$REQ" > /dev/null \
    || fail "latency percentiles and the proxy link must be reported, got: $(jq -c '.data.summary' <<< "$REQ")"
pass "series ≤ 481 buckets, p95 latency reported, proxy connected"
wait_for 60 "server-wide traffic" bash -c "curl -sf --unix-socket '$SOCK' \
    'http://localhost/api/v1/system/requests?range=24h' \
    | jq -e '(.data.busiest | map(.name) | index(\"e2e-whoami\")) != null and .data.unknown_requests >= 1'"
pass "the traffic overview lists e2e-whoami and counts unknown hosts"

[[ "$(api_req PUT /api/v1/apps/e2e-whoami/access-log '{"enabled":true}')" == 200 ]] \
    || fail "enabling the access log failed: $(cat "$API_BODY")"
# The route_state trigger makes the proxy reload; then it writes the lines.
wait_for 30 "an access log line" bash -c "curl -sk -o /dev/null --resolve '${WHOAMI_DOMAIN}:443:${IP}' \
    'https://${WHOAMI_DOMAIN}/e2e-access?token=secret' && curl -sf --unix-socket '$SOCK' \
    'http://localhost/api/v1/apps/e2e-whoami/access-log?q=e2e-access' | jq -e '.data.lines | length >= 1'"
LINE=$(api "/api/v1/apps/e2e-whoami/access-log?q=e2e-access" | jq -r '.data.lines[-1]')
[[ "$LINE" == *" GET /e2e-access 200 "* && "$LINE" != *secret* ]] \
    || fail "access lines must hold method, path and status without the query string, got: ${LINE}"
pass "access log line recorded without the query string: ${LINE}"
[[ "$(perms "${DATA_DIR}/access")" == "700 tond-proxy tond-proxy" ]] \
    || fail "access/ must be 700 tond-proxy, got: $(perms "${DATA_DIR}/access")"
WHOAMI_APP_ID=$(app_field e2e-whoami id)
[[ "$(stat -c '%a %U' "${DATA_DIR}/access/${WHOAMI_APP_ID}.log")" == "600 tond-proxy" ]] \
    || fail "the access log file must be 600 tond-proxy"
pass "access logs live in a proxy-owned 0700 directory, files 0600"
curl -sf --unix-socket "$SOCK" "http://localhost/api/v1/apps/e2e-whoami/access-log/download" | grep -q "/e2e-access" \
    || fail "the access log download must contain the recorded line"
pass "the full access log downloads"
[[ "$(api_req PUT /api/v1/apps/e2e-whoami/access-log '{"enabled":false}')" == 200 ]] \
    || fail "disabling the access log failed"
[[ "$(api_req DELETE /api/v1/apps/e2e-whoami/access-log '')" == 200 ]] || fail "deleting the access log failed"
[[ ! -e "${DATA_DIR}/access/${WHOAMI_APP_ID}.log" ]] || fail "the access log file must be gone after delete"
pass "access log turned off and deleted"

step "Webhook and integration hardening"
API_TCP="http://127.0.0.1:3001"
# status_of <curl args...>: HTTP status code of a request to the TCP API (no credentials).
status_of() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
WHOAMI_ID=$(app_field e2e-whoami id)
[[ "$WHOAMI_ID" =~ ^[0-9a-f-]{36}$ ]] || fail "could not read the e2e-whoami app id (got: ${WHOAMI_ID})"
WEBHOOK_JSON=$(api "/api/v1/apps/${WHOAMI_ID}/webhook")
[[ "$(jq -r '.data.has_secret' <<< "$WEBHOOK_JSON")" == true && "$(jq -r '.data.secret // empty' <<< "$WEBHOOK_JSON")" == "" ]] \
    || fail "the webhook config must say a secret is set without returning it"
WEBHOOK_SECRET=$(api "/api/v1/apps/${WHOAMI_ID}/webhook-secret" | jq -r '.data.secret // empty')
[[ ${#WEBHOOK_SECRET} -ge 32 ]] || fail "every app must get a generated webhook secret"
pass "app has a generated webhook secret (write-only; revealed on request)"
[[ "$(jq -r '.data.paths.github' <<< "$WEBHOOK_JSON")" == "/api/v1/webhooks/github/${WHOAMI_ID}" ]] \
    || fail "webhook URLs must be addressed by app UUID"
pass "webhook URLs are addressed by app UUID"

PUSH_BODY='{"ref":"refs/heads/main","after":"e2e"}'
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -H 'Content-Type: application/json' \
    -d "$PUSH_BODY" "${API_TCP}/api/v1/webhooks/github/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unsigned GitHub webhook must be rejected"
pass "unsigned GitHub webhook → 401"
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -H 'X-Hub-Signature-256: sha256=deadbeef' \
    -d "$PUSH_BODY" "${API_TCP}/api/v1/webhooks/github/${WHOAMI_ID}")" == 401 ]] \
    || fail "a wrongly signed GitHub webhook must be rejected"
pass "wrongly signed GitHub webhook → 401"
[[ "$(status_of -X POST -H 'X-Gitlab-Event: Push Hook' -H 'X-Gitlab-Token: wrong' \
    -d "$PUSH_BODY" "${API_TCP}/api/v1/webhooks/gitlab/${WHOAMI_ID}")" == 401 ]] \
    || fail "a GitLab webhook with a wrong token must be rejected"
pass "GitLab webhook with a wrong token → 401"
[[ "$(status_of -X POST -H 'X-Gitea-Event: push' \
    -d "$PUSH_BODY" "${API_TCP}/api/v1/webhooks/gitea/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unsigned Gitea webhook must be rejected"
pass "unsigned Gitea webhook → 401"
[[ "$(status_of -X POST "${API_TCP}/api/v1/webhooks/generic/${WHOAMI_ID}")" == 401 ]] \
    || fail "an unauthenticated generic webhook must be rejected"
pass "unauthenticated generic webhook → 401"
[[ "$(status_of -X POST -H 'X-GitHub-Event: push' -d "$PUSH_BODY" \
    "${API_TCP}/api/v1/webhooks/github/e2e-whoami")" == 404 ]] \
    || fail "webhooks must not resolve apps by name"
pass "webhook by app name → 404"

# Authenticated generic call: accepted (auto-deploy is off, so nothing deploys),
# and replaying the same delivery ID is refused.
DELIVERY="e2e-$(date +%s)-$$"
generic_signed() {
    status_of -X POST -H "Authorization: Bearer ${WEBHOOK_SECRET}" -H "X-Tond-Delivery: ${DELIVERY}" \
        "${API_TCP}/api/v1/webhooks/generic/${WHOAMI_ID}"
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
        "${API_TCP}/api/v1/webhooks/github/${WHOAMI_ID}")" == 200 ]] \
        || fail "a correctly signed GitHub webhook must be accepted"
    pass "correctly signed GitHub webhook → 200"
else
    echo "  - openssl not installed; skipping the signed GitHub webhook check"
fi

[[ "$(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" -X PUT \
    -H 'Content-Type: application/json' -d '{"secret":null}' \
    "http://localhost/api/v1/apps/${WHOAMI_ID}/webhook-secret")" != 200 ]] \
    || fail "the webhook secret must not be clearable"
pass "webhook secret cannot be cleared"
ROTATED=$(curl -sf --unix-socket "$SOCK" -X POST \
    "http://localhost/api/v1/apps/${WHOAMI_ID}/webhook-secret/rotate" | jq -r '.data.secret // empty')
[[ ${#ROTATED} -ge 32 && "$ROTATED" != "$WEBHOOK_SECRET" ]] || fail "rotating the webhook secret must return a new one"
[[ "$(status_of -X POST -H "Authorization: Bearer ${WEBHOOK_SECRET}" \
    "${API_TCP}/api/v1/webhooks/generic/${WHOAMI_ID}")" == 401 ]] \
    || fail "the old webhook secret must stop working after rotation"
pass "rotated secret replaces the old one"

GH_APPS_BEFORE=$(api /api/v1/github-apps | jq '.data | length')
# GitHub's redirect only hands off to the dashboard; completing needs the
# admin's session (same-origin POST) and their pending, single-use state.
callback=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' \
    "${API_TCP}/api/v1/github-apps/callback?code=e2eattackercode&state=forged")
[[ "$callback" == "303 "*"/integrations/github/complete?kind=manifest"* ]] \
    || fail "the GitHub App callback must hand off to the dashboard (got: ${callback})"
completion=$(curl -s -X POST -H 'Content-Type: application/json' \
    -d '{"kind":"manifest","code":"e2eattackercode"}' "${API_TCP}/api/v1/github-apps/complete")
[[ "$(jq -r .error.message <<< "$completion")" == missing_state ]] \
    || fail "completing a GitHub App flow without state must be refused (got: ${completion})"
completion=$(curl -s -X POST -H 'Content-Type: application/json' \
    -d '{"kind":"manifest","code":"e2eattackercode","state":"forged"}' "${API_TCP}/api/v1/github-apps/complete")
[[ "$(jq -r .error.message <<< "$completion")" == not_logged_in ]] \
    || fail "completing a GitHub App flow without an admin session must be refused (got: ${completion})"
[[ "$(api /api/v1/github-apps | jq '.data | length')" == "$GH_APPS_BEFORE" ]] \
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

step "API token scopes, filters and audit"
# api_as <token> <METHOD> <path> [json]: HTTP status over loopback TCP; body in $API_BODY.
api_as() {
    curl -s -o "$API_BODY" -w '%{http_code}' -X "$2" -H "Authorization: Bearer $1" \
        -H 'Content-Type: application/json' ${4:+-d "$4"} "http://127.0.0.1:3001$3"
}
body_code() { jq -r '.error.code // empty' "$API_BODY"; }
CI_TOKEN=$(tond token create --name e2e-ci --kind ci --preset deployer --app e2e-whoami --expires 1d 2> /dev/null)
[[ "$(api_as "$CI_TOKEN" GET /api/v1/apps/e2e-whoami)" == 200 ]] || fail "the CI token must read its own app"
api_as "$CI_TOKEN" GET /api/v1/apps > /dev/null
[[ "$(jq -r '.data | map(.name) | join(",")' "$API_BODY")" == e2e-whoami ]] \
    || fail "a token limited to e2e-whoami must only list e2e-whoami, got: $(cat "$API_BODY")"
pass "a resource-limited token sees only its app"
code=$(api_as "$CI_TOKEN" GET /api/v1/apps/e2e-static)
[[ "$code" == 403 && "$(body_code)" == RESOURCE_DENIED ]] \
    || fail "the CI token must be refused other apps with 403 RESOURCE_DENIED, got ${code} $(cat "$API_BODY")"
pass "resource filter blocks other apps (403 RESOURCE_DENIED)"
code=$(api_as "$RO_TOKEN" PUT /api/v1/settings '{"server_name":"hijacked"}')
[[ "$code" == 403 && "$(body_code)" == SCOPE_DENIED ]] \
    || fail "a read-only token must get 403 SCOPE_DENIED for settings, got ${code} $(cat "$API_BODY")"
code=$(api_as "$CI_TOKEN" DELETE /api/v1/apps/e2e-whoami)
[[ "$code" == 403 && "$(body_code)" == SCOPE_DENIED ]] \
    || fail "a deployer token must get 403 SCOPE_DENIED for apps:delete, got ${code}"
pass "missing scopes → 403 with the stable code SCOPE_DENIED"

REVOKED=$(tond token create --name e2e-revoked --preset readonly --expires 1d 2> /dev/null)
[[ "$(api_as "$REVOKED" GET /api/v1/apps)" == 200 ]] || fail "a fresh token must work"
tond token revoke e2e-revoked -y > /dev/null
code=$(api_as "$REVOKED" GET /api/v1/apps)
[[ "$code" == 401 && "$(body_code)" == TOKEN_REVOKED ]] || fail "a revoked token must get 401 TOKEN_REVOKED, got ${code}"
pass "revoked token → 401 TOKEN_REVOKED"

step "Stable API: /api/v1, OpenAPI, structured errors, idempotency, CLI contract"
spec=$(curl -sf "${API_TCP}/api/v1/openapi.json") || fail "GET /api/v1/openapi.json must be public"
jq -e '.openapi == "3.1.0"
    and .paths["/api/v1/apps/{id}/deploy"].post["x-tond-scopes"] == ["apps:deploy"]
    and (.components.securitySchemes | has("bearerAuth"))
    and ([.components.schemas.ErrorCode.enum[]] | index("DEPLOY_IN_PROGRESS") != null)' <<< "$spec" > /dev/null \
    || fail "the OpenAPI document must describe the v1 routes, their scopes and the error codes"
pass "OpenAPI 3.1 served at /api/v1/openapi.json ($(jq '[.paths[] | keys[]] | length' <<< "$spec") operations)"

code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" http://localhost/api/apps)
[[ "$code" == 404 && "$(jq -r .error.code "$API_BODY")" == ROUTE_NOT_FOUND ]] \
    || fail "unversioned /api paths must answer 404 ROUTE_NOT_FOUND, got ${code} $(cat "$API_BODY")"
jq -e '.error.hint | contains("/api/v1/apps")' "$API_BODY" > /dev/null \
    || fail "the ROUTE_NOT_FOUND hint must point at /api/v1: $(cat "$API_BODY")"
code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" http://localhost/api/v1/apps/e2e-no-such-app)
[[ "$code" == 404 && "$(jq -r .error.code "$API_BODY")" == APP_NOT_FOUND ]] \
    && jq -e '.success == false and (.error.message | length > 0) and (.error.hint | length > 0)' "$API_BODY" > /dev/null \
    || fail "an unknown app must answer 404 {code: APP_NOT_FOUND, message, hint}, got ${code} $(cat "$API_BODY")"
code=$(api_req POST /api/v1/apps '{"name":"e2e-bad","source_type":"image","container_port":"eighty"}')
[[ "$code" =~ ^4 && "$(jq -r .error.code "$API_BODY")" == INVALID_JSON && "$(jq -r .error.field "$API_BODY")" == container_port ]] \
    || fail "a mistyped field must answer INVALID_JSON naming the field, got ${code} $(cat "$API_BODY")"
code=$(api_req POST /api/v1/apps '{"name":"bad name!","source_type":"image","source_url":"traefik/whoami:v1.10"}')
[[ "$code" == 400 && "$(jq -r .error.code "$API_BODY")" == INVALID_INPUT && "$(jq -r .error.field "$API_BODY")" == name ]] \
    || fail "an invalid app name must answer 400 INVALID_INPUT with field=name, got ${code} $(cat "$API_BODY")"
pass "errors are {success:false, error:{code, message, hint, field}} with stable codes"

# Idempotency-Key: the repeat replays the first response, a different body is refused.
IDEM_KEY="e2e-$(date +%s)-$RANDOM"
idem_post() {
    curl -s -D "$1" -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" -X POST \
        -H 'Content-Type: application/json' -H "Idempotency-Key: ${IDEM_KEY}" -d "$2" \
        http://localhost/api/v1/projects
}
IDEM_HDRS=$(mktemp)
[[ "$(idem_post "$IDEM_HDRS" '{"name":"e2e-idem"}')" == 201 ]] || fail "creating a project with an Idempotency-Key must work: $(cat "$API_BODY")"
first_id=$(jq -r .data.id "$API_BODY")
[[ "$(idem_post "$IDEM_HDRS" '{"name":"e2e-idem"}')" == 201 ]] || fail "the repeat must replay 201: $(cat "$API_BODY")"
[[ "$(jq -r .data.id "$API_BODY")" == "$first_id" ]] || fail "the repeat must return the same project, not create another"
grep -qi '^idempotent-replayed: true' "$IDEM_HDRS" || fail "a replay must carry Idempotent-Replayed: true"
[[ "$(api /api/v1/projects | jq '[.data[] | select(.name == "e2e-idem")] | length')" == 1 ]] \
    || fail "a replayed create must not create a second project"
code=$(idem_post "$IDEM_HDRS" '{"name":"e2e-idem-2"}')
[[ "$code" == 422 && "$(jq -r .error.code "$API_BODY")" == IDEMPOTENCY_KEY_REUSED ]] \
    || fail "the same key with another body must answer 422 IDEMPOTENCY_KEY_REUSED, got ${code}"
curl -s -o /dev/null --unix-socket "$SOCK" -X DELETE "http://localhost/api/v1/projects/${first_id}"
rm -f "$IDEM_HDRS"
pass "Idempotency-Key replays the first response and refuses a different body (422)"

# CLI: JSON output, documented exit codes, --yes.
tond --json apps list | jq -e 'type == "array" and any(.[]; .name == "e2e-whoami")' > /dev/null \
    || fail "tond --json apps list must print a JSON array"
set +e
cli_err=$(tond --json apps info e2e-no-such-app 2>&1 > /dev/null); cli_rc=$?
set -e
[[ "$cli_rc" == 3 && "$(jq -r .error.code <<< "$cli_err")" == APP_NOT_FOUND ]] \
    || fail "tond --json apps info <missing> must exit 3 with APP_NOT_FOUND on stderr, got ${cli_rc}: ${cli_err}"
set +e
tond apps delete e2e-whoami < /dev/null > /dev/null 2>&1; cli_rc=$?
set -e
[[ "$cli_rc" == 2 ]] || fail "a destructive command without --yes and without a terminal must exit 2, got ${cli_rc}"
[[ "$(app_field e2e-whoami name)" == e2e-whoami ]] || fail "a refused delete must not delete the app"
set +e
tond --socket /nonexistent/tond.sock apps delete e2e-whoami < /dev/null > /dev/null 2>&1; cli_rc=$?
set -e
[[ "$cli_rc" == 2 ]] \
    || fail "without --yes the refusal (exit 2) must come before connecting, even with no server socket; got ${cli_rc}"
tond help exit-codes --json | jq -e '.exit_codes | map(.exit_code) | contains([0,1,2,3,4,5,6,7,8])' > /dev/null \
    || fail "tond help exit-codes --json must list exit codes 0-8"
[[ "$(tond completions bash)" == *_tond* ]] || fail "tond completions bash must print a completion script"
pass "CLI: --json output, exit codes (3 not found, 2 refused without --yes), completions"
EXPIRED=$(tond token create --name e2e-expired --preset readonly --expires 1h 2> /dev/null)
expire_sql="UPDATE api_tokens SET expires_at = '2000-01-01T00:00:00Z' WHERE name = 'e2e-expired'"
if command -v sqlite3 > /dev/null; then
    sqlite3 "${DATA_DIR}/tond.db" "$expire_sql"
elif command -v python3 > /dev/null; then
    python3 -c "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(sys.argv[2]); c.commit()" \
        "${DATA_DIR}/tond.db" "$expire_sql"
else
    expire_sql=""
fi
if [[ -n "$expire_sql" ]]; then
    code=$(api_as "$EXPIRED" GET /api/v1/apps)
    [[ "$code" == 401 && "$(body_code)" == TOKEN_EXPIRED ]] || fail "an expired token must get 401 TOKEN_EXPIRED, got ${code}"
    pass "expired token → 401 TOKEN_EXPIRED"
else
    echo "  - neither sqlite3 nor python3 available; skipping the expired-token check"
fi
if tond token create --name e2e-agent --kind agent --preset readonly --expires never > /dev/null 2>&1; then
    fail "agent tokens must expire"
fi
if tond token create --name e2e-agent --kind agent --scope users:admin > /dev/null 2>&1; then
    fail "agent tokens must never hold admin scopes"
fi
pass "agent tokens must expire and can't hold admin scopes"

# The API over loopback TCP sees 127.0.0.1; through the proxy, X-Forwarded-For
# is replaced with the real peer, so it can't be spoofed to pass an allow-list.
IP_OK=$(tond token create --name e2e-ip-ok --preset readonly --allow-ip 127.0.0.1 --expires 1d 2> /dev/null)
IP_OTHER=$(tond token create --name e2e-ip-other --preset readonly --allow-ip 10.255.255.1 --expires 1d 2> /dev/null)
[[ "$(api_as "$IP_OK" GET /api/v1/apps)" == 200 ]] || fail "a token allowed from 127.0.0.1 must work over loopback"
code=$(api_as "$IP_OTHER" GET /api/v1/apps)
[[ "$code" == 403 && "$(body_code)" == IP_NOT_ALLOWED ]] || fail "a token pinned to another IP must get 403 IP_NOT_ALLOWED, got ${code}"
code=$(curl -sk -o /dev/null -w '%{http_code}' --resolve "${DASH}:443:${IP}" -H "Authorization: Bearer ${IP_OTHER}" \
    -H 'X-Forwarded-For: 10.255.255.1' "https://${DASH}/api/v1/apps")
[[ "$code" == 403 ]] || fail "a spoofed X-Forwarded-For through the proxy must not satisfy the allow-list, got ${code}"
pass "allowed-IP filter works and can't be spoofed through the proxy"

ENV_TOKEN=$(tond token create --name e2e-env --scope env:write --app e2e-whoami --expires 1d 2> /dev/null)
[[ "$(api_as "$ENV_TOKEN" PATCH /api/v1/apps/e2e-whoami/env '{"set":{"E2E_TOKEN_AUDIT":"1"},"unset":[]}')" == 200 ]] \
    || fail "an env:write token must patch env vars: $(cat "$API_BODY")"
entry=$(api '/api/v1/audit-log?actor=e2e-env&action=env_update&limit=1' | jq -c '.data.entries[0]')
[[ "$(jq -r '.actor_kind' <<< "$entry")" == token && "$(jq -r '.actor_id' <<< "$entry")" != null ]] \
    || fail "token calls must be audited with the token's id and name as actor, got: ${entry}"
api_as "$ENV_TOKEN" PATCH /api/v1/apps/e2e-whoami/env '{"set":{},"unset":["E2E_TOKEN_AUDIT"]}' > /dev/null
[[ "$(api '/api/v1/audit-log?action=token_create&actor=local-socket&limit=1' | jq -r '.data.entries[0].actor_kind')" == local_socket ]] \
    || fail "tokens created on the host must be audited as local-socket"
pass "token calls are audited as the token (id + name); CLI actions as local-socket"
tond audit verify > /dev/null || fail "the audit log hash chain must verify"
pass "tond audit verify: the hash chain is intact"

step "TLS certificate issuance (ACME HTTP-01)"
if [[ -n "$CA_FILE" ]]; then
    wait_for 240 "a certificate for ${WHOAMI_DOMAIN} signed by ${CA_FILE}" \
        curl -sf --cacert "$CA_FILE" --resolve "${WHOAMI_DOMAIN}:443:${IP}" "https://${WHOAMI_DOMAIN}/"
    pass "${WHOAMI_DOMAIN} serves a certificate that verifies against the ACME CA"
    wait_for 30 "certificate state for ${WHOAMI_DOMAIN} recorded as active" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/certs | jq -e '.data[] | select(.domain == \"${WHOAMI_DOMAIN}\" and .status == \"active\" and .failure_count == 0 and .next_retry_at == null and .last_error == null)'"
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

    read -r code body < <(api_send PUT /api/v1/settings "{\"preview_domain\": \"previews.${BASE}\"}")
    [[ "$code" == 400 ]] || fail "a preview domain on the dashboard's site must be rejected (got ${code}: ${body})"
    pass "a preview domain on the dashboard's site (previews.${BASE}) is rejected"

    read -r code body < <(api_send POST /api/v1/apps/e2e-whoami/preview '{"ttl_hours": 1}')
    [[ "$code" == 202 ]] || fail "creating a preview must succeed (got ${code}: ${body})"
    PREVIEW_ID=$(jq -r .data.id <<< "$body")
    PREVIEW_HOST=$(jq -r .data.subdomain <<< "$body")
    [[ "$PREVIEW_HOST" == *"-e2e-whoami.${PREVIEW}" ]] || fail "preview host must be under ${PREVIEW}, got: ${PREVIEW_HOST}"
    wait_for 180 "preview ${PREVIEW_HOST} running" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/previews | jq -e '.data[] | select(.id == \"${PREVIEW_ID}\" and .status == \"running\")'"
    wait_for 60 "route for ${PREVIEW_HOST}" \
        bash -c "curl -sk --resolve '${PREVIEW_HOST}:443:${IP}' 'https://${PREVIEW_HOST}/' | grep -q Hostname:"
    pass "preview served at https://${PREVIEW_HOST}"

    for host in "$PREVIEW" "dash.${PREVIEW}" "unknown.${PREVIEW}"; do
        status=$(proxied_status "$host" /api/v1/apps)
        [[ "$status" == 404 ]] || fail "https://${host}/api/v1/apps must be 404 (dashboard/API never on the preview domain), got ${status}"
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

    read -r code body < <(api_send PUT /api/v1/settings '{"preview_domain": ""}')
    [[ "$code" == 200 ]] || fail "clearing the preview domain must succeed (got ${code}: ${body})"
    read -r code body < <(api_send POST /api/v1/apps/e2e-whoami/preview '{}')
    [[ "$code" == 400 && "$body" == *"preview domain"* ]] \
        || fail "previews must be refused without a preview domain (got ${code}: ${body})"
    read -r code body < <(api_send PUT /api/v1/settings "{\"preview_domain\": \"${PREVIEW}\"}")
    [[ "$code" == 200 ]] || fail "restoring the preview domain must succeed (got ${code}: ${body})"
    pass "previews are refused with an explanation when no preview domain is set"

    curl -sf --unix-socket "$SOCK" -X DELETE "http://localhost/api/v1/apps/e2e-whoami/previews/${PREVIEW_ID}" > /dev/null \
        || fail "deleting the preview must succeed"
    pass "preview deleted"
else
    echo "  - E2E_PREVIEW_DOMAIN not set; skipping preview checks"
fi

step "Managed database"
tond db create e2e-redis --db-type redis > /dev/null
wait_for 240 "e2e-redis running" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/databases | jq -e '.data[] | select(.name == \"e2e-redis\" and .status == \"running\")'"
pass "e2e-redis is running"
REDIS_DETAIL=$(api /api/v1/databases/e2e-redis)
REDIS_CREDS=$(api /api/v1/databases/e2e-redis/credentials)
REDIS_USER=$(jq -r '.data.db_user' <<<"$REDIS_CREDS")
REDIS_PASS=$(jq -r '.data.db_password' <<<"$REDIS_CREDS")
[[ ${#REDIS_PASS} -ge 16 ]] || fail "the credentials reveal must return the Redis password"
# The server may still be loading right after "running"; retry the auth check briefly.
wait_for 30 "e2e-redis answering authenticated PING" \
    bash -c "docker exec -e REDISCLI_AUTH='${REDIS_PASS}' tond-db-e2e-redis redis-cli --no-auth-warning --user '${REDIS_USER}' PING | grep -qx PONG"
noauth=$(docker exec tond-db-e2e-redis redis-cli PING 2>&1 || true)
[[ "$noauth" == *NOAUTH* ]] || fail "managed Redis must refuse unauthenticated clients, got: ${noauth}"
wrong=$(docker exec -e REDISCLI_AUTH=wrong tond-db-e2e-redis redis-cli --no-auth-warning --user "$REDIS_USER" PING 2>&1 || true)
[[ "$wrong" != *PONG* ]] || fail "managed Redis must refuse a wrong password"
jq -e --arg u "$REDIS_USER" --arg p "$REDIS_PASS" '.data.connection_string | contains($u + ":" + $p + "@")' <<<"$REDIS_CREDS" > /dev/null \
    || fail "the revealed Redis connection string must carry the credentials"
pass "managed Redis requires auth (NOAUTH without, PONG with the ACL user); revealed connection string carries credentials"

step "Secrets at rest and write-only API secrets"
# The database detail never carries the password; its connection string is masked.
[[ "$(jq -r '.data.db_password // empty' <<<"$REDIS_DETAIL")" == "" && "$(jq -r '.data.has_password' <<<"$REDIS_DETAIL")" == true ]] \
    || fail "the database detail must report has_password without the password"
grep -qF "$REDIS_PASS" <<<"$REDIS_DETAIL" && fail "the database detail must not contain the password"
jq -e '.data.connection_string | contains("••••••••")' <<<"$REDIS_DETAIL" > /dev/null \
    || fail "the database detail's connection string must be masked"
pass "database detail: has_password, masked connection string, no password"

# An env var value is stored encrypted: neither the database file (nor its
# WAL) nor the env listing contains it; an explicit reveal returns it.
CANARY="e2e-canary-$(date +%s)-${RANDOM}${RANDOM}"
[[ "$(api_req PATCH "/api/v1/apps/${WHOAMI_ID}/env" "{\"set\":{\"E2E_CANARY\":\"${CANARY}\"}}")" == 200 ]] \
    || fail "setting an env var must succeed: $(cat "$API_BODY")"
grep -qF "$CANARY" "$API_BODY" && fail "the env patch response must not echo the value"
api "/api/v1/apps/${WHOAMI_ID}/env" | grep -qF "$CANARY" && fail "the env listing must mask values"
[[ "$(api "/api/v1/apps/${WHOAMI_ID}/env/E2E_CANARY" | jq -r '.data.value')" == "$CANARY" ]] \
    || fail "revealing an env var must return its value"
for f in "${DATA_DIR}"/tond.db*; do
    grep -qaF "$CANARY" "$f" && fail "env var value found in plaintext in ${f}"
    grep -qaF "$REDIS_PASS" "$f" && fail "database password found in plaintext in ${f}"
    grep -qaF "$WEBHOOK_SECRET" "$f" && fail "webhook secret found in plaintext in ${f}"
done
pass "env values, database passwords and webhook secrets are not stored in plaintext"

for action in env_reveal webhook_secret_reveal db_credentials_reveal; do
    api "/api/v1/audit-log?action=${action}&limit=1" | jq -e '.data.entries | length > 0' > /dev/null \
        || fail "a ${action} must be audit-logged"
done
pass "every secret reveal is audit-logged"

step "Per-database backup"
[[ "$(api_req POST /api/v1/databases/e2e-redis/backups '{}')" == 201 ]] \
    || fail "backing up a database must succeed: $(cat "$API_BODY")"
[[ "$(jq -r '.data.status + " " + .data.scope' "$API_BODY")" == "completed database:e2e-redis" ]] \
    || fail "a database backup must complete with the database scope: $(cat "$API_BODY")"
DB_BACKUP_ID=$(jq -r '.data.id' "$API_BODY")
api /api/v1/databases/e2e-redis/backups | jq -e --arg id "$DB_BACKUP_ID" '.data[] | select(.id == $id)' > /dev/null \
    || fail "the database backup must be listed for the database"
[[ -d "${DATA_DIR}/backups/snapshots/${DB_BACKUP_ID}/databases/e2e-redis" ]] \
    || fail "the database backup must hold the database's data"
pass "database backed up and listed"
[[ "$(api_req POST "/api/v1/snapshots/${DB_BACKUP_ID}/restore-database" '{"db_name":"e2e-redis"}')" == 200 ]] \
    || fail "restoring the database must succeed: $(cat "$API_BODY")"
SAFETY_ID=$(jq -r '.data.pre_restore_snapshot_id // empty' "$API_BODY")
api /api/v1/snapshots | jq -e --arg id "$SAFETY_ID" '.data[] | select(.id == $id and .scope == "database:e2e-redis" and .trigger == "pre_restore")' > /dev/null \
    || fail "a database restore must back up the current data first"
wait_for 60 "e2e-redis answering after the restore" \
    bash -c "docker exec -e REDISCLI_AUTH='${REDIS_PASS}' tond-db-e2e-redis redis-cli --no-auth-warning --user '${REDIS_USER}' PING | grep -qx PONG"
pass "database restored from its backup (safety-net backup taken first)"

step "Scheduled database backups and restore into a new database"
# A schedule that keeps no backups is refused with a structured error.
[[ "$(api_req PUT /api/v1/databases/e2e-redis/backup-schedule '{"schedule":"0 3 * * *","retention":0}')" == 400 ]] \
    || fail "a backup schedule keeping zero backups must be refused: $(cat "$API_BODY")"
[[ "$(jq -r '.error.code' "$API_BODY")" == INVALID_INPUT ]] || fail "expected INVALID_INPUT: $(cat "$API_BODY")"
for body in '{"full_retention_count":0}' '{"pre_deploy_retention_per_app":0}'; do
    [[ "$(api_req PUT /api/v1/snapshots/config "$body")" == 400 ]] \
        || fail "a snapshot retention of 0 must be refused (${body}): $(cat "$API_BODY")"
done
pass "retention counts below one are refused (database schedule, full and pre-deploy snapshots)"
# Data to find again in the restored copy.
docker exec -e REDISCLI_AUTH="${REDIS_PASS}" tond-db-e2e-redis redis-cli --no-auth-warning --user "${REDIS_USER}" \
    SET e2e-restore-key restored-ok > /dev/null
[[ "$(api_req PUT /api/v1/databases/e2e-redis/backup-schedule '{"schedule":"* * * * *","retention":2}')" == 200 ]] \
    || fail "scheduling database backups must succeed: $(cat "$API_BODY")"
jq -e '.data.schedule == "* * * * *" and .data.retention == 2 and (.data.next_run_at | type == "string")' "$API_BODY" > /dev/null \
    || fail "the schedule must report its next run: $(cat "$API_BODY")"
wait_for 150 "a scheduled backup of e2e-redis" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/databases/e2e-redis/backups' | jq -e '.data[] | select(.scope == \"database:e2e-redis\" and .trigger == \"scheduled\" and .status == \"completed\")'"
SCHED_ID=$(api /api/v1/databases/e2e-redis/backups \
    | jq -r '[.data[] | select(.scope == "database:e2e-redis" and .trigger == "scheduled" and .status == "completed")][0].id')
[[ -f "${DATA_DIR}/backups/snapshots/${SCHED_ID}/databases/e2e-redis/meta.json" ]] \
    || fail "a database backup must describe the database (meta.json) for restores into a new database"
[[ "$(api_req PUT /api/v1/databases/e2e-redis/backup-schedule '{"schedule":null}')" == 200 ]] \
    || fail "turning the schedule off must succeed: $(cat "$API_BODY")"
pass "a scheduled backup ran from the database's own schedule (retention 0 refused)"

[[ "$(api_req POST "/api/v1/snapshots/${SCHED_ID}/restore-new-database" '{"source":"e2e-redis","name":"e2e-redis-copy"}')" == 201 ]] \
    || fail "restoring into a new database must succeed: $(cat "$API_BODY")"
COPY_CREDS=$(api /api/v1/databases/e2e-redis-copy/credentials)
COPY_USER=$(jq -r '.data.db_user' <<<"$COPY_CREDS")
COPY_PASS=$(jq -r '.data.db_password' <<<"$COPY_CREDS")
[[ "$COPY_PASS" != "$REDIS_PASS" ]] || fail "the restored copy must get its own password"
wait_for 60 "e2e-redis-copy holding the backed-up key" \
    bash -c "docker exec -e REDISCLI_AUTH='${COPY_PASS}' tond-db-e2e-redis-copy redis-cli --no-auth-warning --user '${COPY_USER}' GET e2e-restore-key | grep -qx restored-ok"
docker exec -e REDISCLI_AUTH="${REDIS_PASS}" tond-db-e2e-redis redis-cli --no-auth-warning --user "${REDIS_USER}" PING | grep -qx PONG \
    || fail "the original database must keep running during a restore into a new database"
pass "redis backup restored into e2e-redis-copy with its own credentials; e2e-redis untouched"

# PostgreSQL: a row survives backup → restore into a new database (new owner).
tond db create e2e-pg --db-type postgres > /dev/null
wait_for 240 "e2e-pg running" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/databases/e2e-pg | jq -e '.data.status == \"running\" and .data.version == \"16\"'"
PG_CREDS=$(api /api/v1/databases/e2e-pg/credentials)
PG_USER=$(jq -r '.data.db_user' <<<"$PG_CREDS")
wait_for 60 "e2e-pg accepting SQL" docker exec tond-db-e2e-pg psql -U "$PG_USER" -d "$PG_USER" -c 'SELECT 1'
docker exec tond-db-e2e-pg psql -v ON_ERROR_STOP=1 -U "$PG_USER" -d "$PG_USER" \
    -c "CREATE TABLE e2e(id int PRIMARY KEY, v text); INSERT INTO e2e VALUES (1, 'restored-ok');" > /dev/null
[[ "$(api_req POST /api/v1/databases/e2e-pg/extensions '{"name":"pg_trgm"}')" == 200 ]] \
    || fail "enabling an offered extension must succeed: $(cat "$API_BODY")"
api /api/v1/databases/e2e-pg/extensions | jq -e '.data[] | select(.name == "pg_trgm" and .installed_version != null)' > /dev/null \
    || fail "pg_trgm must be listed as enabled"
[[ "$(api_req POST /api/v1/databases/e2e-pg/extensions '{"name":"plpython3u"}')" == 400 ]] \
    || fail "extensions outside the offered list must be refused"
[[ "$(api_req POST /api/v1/databases/e2e-pg/backups '{}')" == 201 ]] || fail "backing up e2e-pg must succeed: $(cat "$API_BODY")"
PG_BACKUP_ID=$(jq -r '.data.id' "$API_BODY")
[[ -s "${DATA_DIR}/backups/snapshots/${PG_BACKUP_ID}/databases/e2e-pg/dump.archive" ]] \
    || fail "a postgres backup must hold a pg_dump archive"
[[ "$(api_req POST "/api/v1/snapshots/${PG_BACKUP_ID}/restore-new-database" '{"source":"e2e-pg","name":"e2e-pg-copy"}')" == 201 ]] \
    || fail "restoring e2e-pg into a new database must succeed: $(cat "$API_BODY")"
PGC_USER=$(api /api/v1/databases/e2e-pg-copy/credentials | jq -r '.data.db_user')
[[ "$(docker exec tond-db-e2e-pg-copy psql -At -U "$PGC_USER" -d "$PGC_USER" -c 'SELECT v FROM e2e WHERE id = 1')" == restored-ok ]] \
    || fail "the restored postgres copy must hold the backed-up row"
[[ "$(docker exec tond-db-e2e-pg-copy psql -At -U "$PGC_USER" -d "$PGC_USER" -c "SELECT tableowner FROM pg_tables WHERE tablename = 'e2e'")" == "$PGC_USER" ]] \
    || fail "restored tables must belong to the new database's own user"
pass "postgres row restored into e2e-pg-copy (owned by its own user); pg_trgm enabled through the API"

step "Database major-version upgrade and public access allowlist"
[[ "$(api_req POST /api/v1/databases/e2e-pg-copy/upgrade '{"version":"15"}')" == 400 ]] \
    || fail "a downgrade must be refused: $(cat "$API_BODY")"
[[ "$(api_req POST /api/v1/databases/e2e-pg-copy/upgrade '{"version":"17"}')" == 202 ]] \
    || fail "starting an upgrade must succeed: $(cat "$API_BODY")"
wait_for 600 "the e2e-pg-copy upgrade waiting for confirmation" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/databases/e2e-pg-copy | jq -e '.data.upgrade.status == \"awaiting_confirmation\" or .data.upgrade.status == \"failed\"'"
UPGRADE=$(api /api/v1/databases/e2e-pg-copy | jq -c '.data.upgrade')
[[ "$(jq -r '.status' <<<"$UPGRADE")" == awaiting_confirmation ]] || fail "the upgrade failed: ${UPGRADE}"
[[ "$(docker exec tond-db-e2e-pg-copy psql -At -U "$PGC_USER" -d "$PGC_USER" -c 'SHOW server_version_num' | cut -c1-2)" == 17 ]] \
    || fail "e2e-pg-copy must run PostgreSQL 17 after the upgrade"
[[ "$(docker exec tond-db-e2e-pg-copy psql -At -U "$PGC_USER" -d "$PGC_USER" -c 'SELECT v FROM e2e WHERE id = 1')" == restored-ok ]] \
    || fail "the upgraded database must keep its data"
[[ -d "$(jq -r '.rollback_dir' <<<"$UPGRADE")" ]] || fail "the old data directory must be kept until confirmed"
[[ "$(api_req POST /api/v1/databases/e2e-pg-copy/upgrade/confirm '{}')" == 200 ]] || fail "confirming must succeed: $(cat "$API_BODY")"
[[ ! -d "$(jq -r '.rollback_dir' <<<"$UPGRADE")" ]] || fail "confirming must delete the old data directory"
pass "e2e-pg-copy upgraded 16 → 17 with its data (downgrade refused, old data kept until confirmed)"

# The allowlist lands in the DOCKER-USER chain (TOND-DB) and goes away again.
[[ "$(api_req PATCH /api/v1/databases/e2e-pg-copy '{"public_access":true,"allowlist":["192.0.2.10","198.51.100.0/24"]}')" == 200 ]] \
    || fail "making a database public with an allowlist must succeed: $(cat "$API_BODY")"
PGC_PORT=$(jq -r '.data.host_port' "$API_BODY")
jq -e '.data.allowlist_enforced == true and .data.access_warning == null' "$API_BODY" > /dev/null \
    || fail "the allowlist must be enforced: $(cat "$API_BODY")"
iptables -S DOCKER-USER | grep -q -- "-j TOND-DB" || fail "DOCKER-USER must jump to TOND-DB"
iptables -S TOND-DB | grep -- "--ctorigdstport ${PGC_PORT} " | grep -- "-s 192.0.2.10/32" | grep -q -- "-j RETURN" \
    || fail "the allowlist must accept 192.0.2.10 on port ${PGC_PORT}: $(iptables -S TOND-DB)"
iptables -S TOND-DB | grep -- "--ctorigdstport ${PGC_PORT} " | grep -v -- " -s " | grep -q -- "-j DROP" \
    || fail "other sources must be dropped: $(iptables -S TOND-DB)"
[[ "$(api_req PATCH /api/v1/databases/e2e-pg-copy '{"public_access":false,"allowlist":[]}')" == 200 ]] \
    || fail "making the database private again must succeed: $(cat "$API_BODY")"
! iptables -S TOND-DB | grep -q -- "--ctorigdstport ${PGC_PORT} " || fail "a private database must leave no allowlist rules"
pass "allowlist enforced in TOND-DB for the public port, removed when private"
tond db delete e2e-redis-copy -y > /dev/null
tond db delete e2e-pg-copy -y > /dev/null
tond db delete e2e-pg -y > /dev/null

step "Cron job"
tond cron create e2e-cron --schedule "0 0 1 1 *" --image alpine:3 --command "echo tond-e2e-cron-ok" > /dev/null
CRON_ID=$(api /api/v1/cron | jq -r '.data[] | select(.name == "e2e-cron") | .id')
tond cron trigger e2e-cron > /dev/null
wait_for 180 "a successful e2e-cron run" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/cron/${CRON_ID}/runs' | jq -e '.data[] | select(.status == \"success\" and (.output // \"\" | contains(\"tond-e2e-cron-ok\")))'"
pass "manual cron run succeeded with the expected output"

step "Cron schedules, alert channels and first-run checklist (dashboard UX)"
# Next runs come from the scheduler's own parser: standard day-of-week numbering
# (1-5 = Monday-Friday) evaluated in the job's time zone.
PREVIEW=$(api '/api/v1/cron/schedule-preview?schedule=0%209%20*%20*%201-5&timezone=Europe/Berlin&count=5')
[[ "$(jq -r '.data.next_runs | length' <<<"$PREVIEW")" == 5 ]] || fail "schedule preview should list 5 runs: $PREVIEW"
for run in $(jq -r '.data.next_runs[]' <<<"$PREVIEW"); do
    [[ "$(TZ=Europe/Berlin date -d "$run" +%u)" -le 5 ]] || fail "1-5 must mean Monday-Friday, got $run"
    [[ "$(TZ=Europe/Berlin date -d "$run" +%H:%M)" == "09:00" ]] || fail "runs should be 09:00 Berlin time, got $run"
done
[[ "$(api_req GET '/api/v1/cron/schedule-preview?schedule=0%209%20*%20*%20*&timezone=Mars/Olympus' '')" == 400 ]] \
    || fail "an unknown time zone must be rejected"
TZ_CODE=$(api_req POST /api/v1/cron '{"name":"e2e-bad-tz","schedule":"0 3 * * *","image":"alpine:3","timezone":"Mars/Olympus"}')
[[ "$TZ_CODE" == 400 ]] || fail "creating a cron job with an unknown time zone should be 400, got $TZ_CODE"
pass "schedule preview uses Mon-Fri for 1-5 in the job's time zone; unknown zones are rejected"

NO_CHANNEL=$(api_req POST /api/v1/alerts '{"name":"e2e-nobody","kind":"deploy_failed"}')
[[ "$NO_CHANNEL" == 400 ]] || fail "an alert rule that notifies nobody should be 400, got $NO_CHANNEL"
CH_CODE=$(api_req POST /api/v1/integrations '{"name":"e2e-unreachable","kind":"webhook","config":{"webhook_url":"http://127.0.0.1:9/unreachable"}}')
[[ "$CH_CODE" == 201 ]] || fail "a webhook channel should be created, got $CH_CODE: $(cat "$API_BODY")"
UNREACHABLE_ID=$(jq -r '.data.id' "$API_BODY")
RULE_CODE=$(api_req POST /api/v1/alerts "{\"name\":\"e2e-deploys\",\"kind\":\"deploy_failed\",\"integration_ids\":[\"${UNREACHABLE_ID}\"]}")
[[ "$RULE_CODE" == 201 ]] || fail "a deploy-failed rule with a channel should be created, got $RULE_CODE: $(cat "$API_BODY")"
RULE_ID=$(jq -r '.data.id' "$API_BODY")
TEST_CODE=$(api_req POST "/api/v1/alerts/${RULE_ID}/test" '')
[[ "$TEST_CODE" == 200 ]] || fail "send-test should answer 200, got $TEST_CODE"
jq -e '.data | length == 1 and .[0].kind == "webhook" and .[0].ok == false and (.[0].error | length > 0)' "$API_BODY" > /dev/null \
    || fail "send-test should report the unreachable webhook: $(cat "$API_BODY")"
api /api/v1/integrations | jq -e --arg id "$UNREACHABLE_ID" '.data[] | select(.id == $id and (.last_error | length > 0))' > /dev/null \
    || fail "a failed send must be visible on the channel"
api_req DELETE "/api/v1/alerts/${RULE_ID}" '' > /dev/null
api_req DELETE "/api/v1/integrations/${UNREACHABLE_ID}" '' > /dev/null
pass "alert rules need a channel; send-test reports each channel; failures show on the channel"

ONBOARDING=$(api /api/v1/onboarding/status)
jq -e '.data.has_successful_deploy == true and .data.last_deployment.app_name != null and (.data | has("has_source_integration") and has("has_notification_channel"))' \
    <<<"$ONBOARDING" > /dev/null || fail "onboarding status should report the checklist and last deploy: $ONBOARDING"
pass "onboarding status reports the first-run checklist and the latest deployment"

step "Alert engine: an alert fires, notifies a signed webhook and resolves"
# A tiny local receiver stores every POST body (and its signature header);
# a memory rule on e2e-whoami at a near-zero threshold fires on the next
# metrics tick, then raising the threshold resolves it.
if command -v python3 > /dev/null; then
    HOOK_DIR=$(mktemp -d)
    python3 -c '
import http.server, os, sys
d = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        n = len([f for f in os.listdir(d) if f.endswith(".json")])
        open(os.path.join(d, "%03d.sig" % n), "w").write(self.headers.get("X-Tond-Signature-256", ""))
        open(os.path.join(d, "%03d.json" % n), "wb").write(body)
        self.send_response(200)
        self.end_headers()
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", 18099), H).serve_forever()
' "$HOOK_DIR" &
    HOOK_PID=$!
    wait_for 10 "the local webhook receiver" bash -c "curl -s -o /dev/null -X POST -d '{}' http://127.0.0.1:18099/probe"
    rm -f "$HOOK_DIR"/*
    [[ "$(api_req POST /api/v1/integrations '{"name":"e2e-signed-hook","kind":"webhook","config":{"webhook_url":"http://127.0.0.1:18099/hook","signing_secret":"e2e-signing-secret"}}')" == 201 ]] \
        || fail "the signed webhook channel should be created: $(cat "$API_BODY")"
    HOOK_ID=$(jq -r '.data.id' "$API_BODY")
    jq -e '.data.saved_secrets | index("signing_secret") and index("webhook_url")' "$API_BODY" > /dev/null \
        || fail "the channel's secrets must be write-only: $(cat "$API_BODY")"
    [[ "$(api_req POST /api/v1/alerts "{\"name\":\"e2e-memory\",\"kind\":\"memory\",\"app_id\":\"${WHOAMI_ID}\",\"threshold\":0.001,\"for_secs\":0,\"severity\":\"critical\",\"integration_ids\":[\"${HOOK_ID}\"]}")" == 201 ]] \
        || fail "the memory rule should be created: $(cat "$API_BODY")"
    MEM_RULE=$(jq -r '.data.id' "$API_BODY")
    wait_for 120 "the firing notification" bash -c "grep -l '\"event\":\"firing\"' '$HOOK_DIR'/*.json"
    FIRED=$(grep -l '"event":"firing"' "$HOOK_DIR"/*.json | head -1)
    jq -e --arg rule "$MEM_RULE" '.version == 1 and .alert.rule_id == $rule and .alert.target_label == "e2e-whoami" and .alert.severity == "critical" and .alert.value > 0' "$FIRED" > /dev/null \
        || fail "the firing payload must describe the alert: $(cat "$FIRED")"
    if command -v openssl > /dev/null; then
        EXPECTED="sha256=$(openssl dgst -sha256 -hmac e2e-signing-secret -hex < "$FIRED" | awk '{print $NF}')"
        [[ "$(cat "${FIRED%.json}.sig")" == "$EXPECTED" ]] || fail "the webhook signature must be HMAC-SHA256 of the body"
    fi
    api /api/v1/alerts/active | jq -e --arg rule "$MEM_RULE" '.data[] | select(.rule_id == $rule and .state == "firing" and .target == "app:'"${WHOAMI_ID}"'")' > /dev/null \
        || fail "the alert must be listed as firing"
    pass "memory alert fired and a signed notification arrived"

    [[ "$(api_req PATCH "/api/v1/alerts/${MEM_RULE}" '{"threshold":100}')" == 200 ]] || fail "raising the threshold should work: $(cat "$API_BODY")"
    wait_for 120 "the resolved notification" bash -c "grep -l '\"event\":\"resolved\"' '$HOOK_DIR'/*.json"
    api "/api/v1/alerts/history?rule_id=${MEM_RULE}" | jq -e '.data.total == 1 and .data.episodes[0].state == "resolved" and .data.episodes[0].notifications_sent >= 2' > /dev/null \
        || fail "history must show the resolved episode with its notifications: $(api "/api/v1/alerts/history?rule_id=${MEM_RULE}")"
    [[ "$(api /api/v1/alerts/active | jq --arg rule "$MEM_RULE" '[.data[] | select(.rule_id == $rule)] | length')" == 0 ]] \
        || fail "a resolved alert must leave the active list"
    api_req DELETE "/api/v1/alerts/${MEM_RULE}" '' > /dev/null
    api_req DELETE "/api/v1/integrations/${HOOK_ID}" '' > /dev/null
    kill "$HOOK_PID" 2> /dev/null || true
    rm -rf "$HOOK_DIR"
    pass "the alert resolved, sent a resolve notification and is in the history"
else
    echo "  - python3 not available; skipping the alert fire/resolve check"
fi

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
    jq -e '(.CapDrop // []) | any(.[]; ltrimstr("CAP_") == "NET_RAW")' <<<"$hc" > /dev/null \
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
REDIS_PORT=$(api /api/v1/databases | jq -r '.data[] | select(.name == "e2e-redis") | .host_port')
for port in "$WHOAMI_PORT" "$REDIS_PORT"; do
    if ss -Hltn "sport = :${port}" | awk '{print $4}' | grep -vqE '^127\.0\.0\.1:'; then
        ss -ltn "sport = :${port}" >&2
        fail "port ${port} is listening on a public interface"
    fi
done
pass "published ports ${WHOAMI_PORT} and ${REDIS_PORT} only listen on 127.0.0.1"
[[ "$(api /api/v1/databases | jq -r '.data[] | select(.name == "e2e-redis") | .public_access')" == false ]] \
    || fail "managed databases must be private by default"
pass "managed databases are private by default"

jq -e '.["log-opts"]["max-size"] and .["log-opts"]["max-file"]' /etc/docker/daemon.json > /dev/null \
    || fail "install.sh must set default log rotation in /etc/docker/daemon.json"
pass "daemon.json sets default container log rotation"

code=$(api_req POST /api/v1/apps '{"name":"e2e-rooted","source_type":"image","source_url":"alpine:3","container_port":80,"volumes":[{"container_path":"/host","host_path":"/"}]}')
[[ "$code" == 400 ]] && grep -q "is not allowed" "$API_BODY" \
    || fail "a host_path of / must be rejected with 400, got ${code}: $(cat "$API_BODY")"
[[ -z "$(app_field e2e-rooted id)" ]] || fail "no app may be created for a rejected volume"
pass "POST /api/v1/apps with host_path \"/\" → 400, nothing created"
code=$(api_req POST /api/v1/apps/e2e-whoami/volumes '{"container_path":"/sock","host_path":"/var/run/docker.sock"}')
[[ "$code" == 400 ]] || fail "binding docker.sock into an app must be rejected, got ${code}: $(cat "$API_BODY")"
pass "POST /api/v1/apps/{id}/volumes with docker.sock → 400"
code=$(api_req POST /api/v1/apps '{"name":"e2e-compose","source_type":"compose","container_port":80,"compose_content":"services:\n  web:\n    image: nginx:alpine\n    privileged: true\n    network_mode: host\n"}')
[[ "$code" == 400 ]] && grep -q "privileged" "$API_BODY" && grep -q "network_mode: host" "$API_BODY" \
    || fail "a privileged compose file must be rejected with every problem listed, got ${code}: $(cat "$API_BODY")"
pass "compose with privileged + network_mode: host → 400 listing both problems"

step "Network isolation (per-app networks, database links)"
# Every app and database has its own Docker network (none is on the default
# bridge); a database is reachable from outside its project only through a
# link. A throwaway probe container on an app's network sees exactly what the
# app sees.
networks_of() { docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$1"; }
ip_on() { docker inspect -f "{{(index .NetworkSettings.Networks \"$2\").IPAddress}}" "$1"; }
PROBE_IMAGE="redis:7-alpine" # already pulled for e2e-redis; has wget, redis-cli, timeout
# Traffic between networks is dropped, so a refused probe would hang in TCP
# connect: every tool gets its own timeout, --init makes signals work (a
# process running as PID 1 ignores SIGTERM), and the host caps the whole run.
probe() {
    local net=$1; shift
    timeout 30 docker run --rm --init --network "$net" "$PROBE_IMAGE" "$@"
}
http_ok() { probe "$1" wget -q -T 3 -O /dev/null "http://$2/" > /dev/null 2>&1; }
redis_reachable() { probe "$1" redis-cli -t 3 -h "$2" PING 2>&1 | grep -q NOAUTH; }
redis_unreachable() { ! redis_reachable "$@"; }

WHOAMI_NET=tond-net-app-e2e-whoami
STATIC_NET=tond-net-app-e2e-static
REDIS_NET=tond-net-db-e2e-redis
for pair in "tond-e2e-whoami ${WHOAMI_NET}" "tond-e2e-static ${STATIC_NET}" "tond-db-e2e-redis ${REDIS_NET}"; do
    read -r container net <<<"$pair"
    [[ "$(networks_of "$container")" == "${net} " ]] \
        || fail "${container} must be on ${net} only, not the default bridge (on: $(networks_of "$container"))"
done
pass "e2e-whoami, e2e-static and e2e-redis each run on their own network (never the default bridge)"
jq -e '.icc == false' /etc/docker/daemon.json > /dev/null \
    || fail "install.sh must set icc=false for Docker's default bridge"
pass "daemon.json turns inter-container traffic off on the default bridge"
[[ "$(docker network inspect -f '{{index .Options "com.docker.network.bridge.enable_icc"}}' "$REDIS_NET")" == false ]] \
    || fail "a database's own network must have inter-container traffic off"
pass "a database's own network has icc off"

WHOAMI_IP=$(ip_on tond-e2e-whoami "$WHOAMI_NET")
REDIS_IP=$(ip_on tond-db-e2e-redis "$REDIS_NET")
http_ok "$WHOAMI_NET" "$WHOAMI_IP" || fail "the probe must reach e2e-whoami from its own network (probe broken?)"
http_ok "$WHOAMI_NET" "e2e-whoami" || fail "an app must be reachable by name on its own network"
pass "control: e2e-whoami answers on its own network (by IP and by name)"
! http_ok "$STATIC_NET" "$WHOAMI_IP" || fail "e2e-static's network must not reach e2e-whoami (${WHOAMI_IP})"
pass "another app's network cannot reach e2e-whoami's container IP"
! redis_reachable "$WHOAMI_NET" "$REDIS_IP" || fail "an unlinked app must not reach e2e-redis (${REDIS_IP})"
pass "an unlinked app cannot reach a database outside its project"

read -r code body < <(api_send PUT /api/v1/databases/e2e-redis/links/e2e-whoami '{}')
[[ "$code" == 200 ]] && jq -e '.data.linked_apps | any(.name == "e2e-whoami")' <<<"$body" > /dev/null \
    || fail "linking e2e-whoami to e2e-redis must succeed (got ${code}: ${body})"
wait_for 20 "e2e-redis reachable by name from e2e-whoami's network" redis_reachable "$WHOAMI_NET" e2e-redis
pass "after linking, e2e-whoami reaches e2e-redis by name"
api /api/v1/apps/e2e-whoami/databases | jq -e '.data | any(.name == "e2e-redis" and .via == "link")' > /dev/null \
    || fail "GET /api/v1/apps/e2e-whoami/databases must list the linked database"
read -r code body < <(api_send PUT /api/v1/databases/e2e-redis/links/e2e-static '{}')
[[ "$code" == 200 ]] || fail "linking e2e-static must succeed (got ${code}: ${body})"
! http_ok "$STATIC_NET" "$WHOAMI_IP" \
    || fail "two apps linked to the same database must still not reach each other"
pass "apps linked to the same database still cannot reach each other"
read -r code body < <(api_send DELETE /api/v1/databases/e2e-redis/links/e2e-static '{}')
[[ "$code" == 200 ]] || fail "unlinking e2e-static must succeed (got ${code}: ${body})"
read -r code body < <(api_send DELETE /api/v1/databases/e2e-redis/links/e2e-whoami '{}')
[[ "$code" == 200 ]] || fail "unlinking e2e-whoami must succeed (got ${code}: ${body})"
wait_for 30 "e2e-redis unreachable from e2e-whoami after unlinking" \
    redis_unreachable "$WHOAMI_NET" "$REDIS_IP"
[[ "$(networks_of tond-db-e2e-redis)" == "${REDIS_NET} " ]] \
    || fail "unlinking must detach e2e-redis from the apps' networks (on: $(networks_of tond-db-e2e-redis))"
pass "unlinking detaches the database again"
code=$(api_req POST /api/v1/apps '{"name":"e2e-compose","source_type":"compose","container_port":80,"compose_content":"services:\n  web:\n    image: nginx:alpine\nnetworks:\n  shared:\n    external: true\n    name: tond-net-db-e2e-redis\n"}')
[[ "$code" == 400 ]] && grep -q "external" "$API_BODY" \
    || fail "a compose file joining another network must be rejected, got ${code}: $(cat "$API_BODY")"
pass "compose files cannot join another app's or database's network"

step "Upload safety (archives are extracted in Rust, with limits)"
UPLOAD_TMP=$(mktemp -d)
ln -s /etc/shadow "${UPLOAD_TMP}/shadow"
tar -czf "${UPLOAD_TMP}/evil.tar.gz" -C "$UPLOAD_TMP" shadow
code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" \
    -F "file=@${UPLOAD_TMP}/evil.tar.gz" http://localhost/api/v1/apps/e2e-static/upload)
[[ "$code" == 400 ]] && grep -q "outside the project" "$API_BODY" \
    || fail "an archive with a symlink to /etc/shadow must be refused with 400, got ${code}: $(cat "$API_BODY")"
pass "an archive linking outside the project is refused (400, says why)"
read -r code body < <(api_send PUT /api/v1/settings '{"upload_max_bytes": 1048576}')
[[ "$code" == 200 ]] || fail "setting a 1 MiB upload limit must succeed (got ${code}: ${body})"
head -c 3145728 /dev/zero > "${UPLOAD_TMP}/zeros.bin"
tar -czf "${UPLOAD_TMP}/bomb.tar.gz" -C "$UPLOAD_TMP" zeros.bin
code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" \
    -F "file=@${UPLOAD_TMP}/bomb.tar.gz" http://localhost/api/v1/apps/e2e-static/upload)
read -r restore_code _ < <(api_send PUT /api/v1/settings '{"upload_max_bytes": 1073741824}')
[[ "$restore_code" == 200 ]] || fail "restoring the upload limit must succeed"
[[ "$code" == 413 ]] && grep -q "tondignore" "$API_BODY" \
    || fail "an archive that unpacks past the limit must be refused with 413, got ${code}: $(cat "$API_BODY")"
pass "an archive that unpacks past upload_max_bytes is refused (413, says how to fix it)"
[[ -z "$(ls -A "${DATA_DIR}/uploads" 2> /dev/null)" ]] || fail "refused uploads must not leave files in ${DATA_DIR}/uploads"
pass "refused uploads leave nothing behind"
rm -rf "$UPLOAD_TMP"
[[ "$(app_field e2e-static status)" == running ]] || fail "refused uploads must not touch the running app"

step "Apps domain (automatic hostnames never on the dashboard's site)"
api /api/v1/apps/e2e-whoami | jq -e '.data.domain_warning | strings | contains("same site as your dashboard")' > /dev/null \
    || fail "a custom domain on the dashboard's site (${WHOAMI_DOMAIN}) must carry a warning"
pass "custom domain ${WHOAMI_DOMAIN} (same site as ${DASH}) is allowed with a warning"
read -r code body < <(api_send PUT /api/v1/settings "{\"apps_domain\": \"apps.${BASE}\"}")
[[ "$code" == 400 ]] || fail "an apps domain on the dashboard's site must be rejected (got ${code}: ${body})"
pass "an apps domain on the dashboard's site (apps.${BASE}) is rejected"
if [[ -n "$APPS_DOMAIN" ]]; then
    # Capture first: grep -q exiting early would SIGPIPE tond under pipefail.
    apps_settings_out="$(tond settings)"
    grep -qE "^apps-domain +${APPS_DOMAIN}$" <<< "$apps_settings_out" \
        || fail "install.sh must set the apps domain to ${APPS_DOMAIN}"
    tond apps create e2e-auto --source-type image --source traefik/whoami:v1.10 --port 80 > /dev/null
    tond apps deploy e2e-auto > /dev/null
    wait_for_app_running e2e-auto 180
    [[ "$(app_field e2e-auto domain)" == "e2e-auto.${APPS_DOMAIN}" ]] \
        || fail "an app without a domain must get e2e-auto.${APPS_DOMAIN}, got $(app_field e2e-auto domain)"
    wait_for 60 "route for e2e-auto.${APPS_DOMAIN}" \
        bash -c "curl -sk --resolve 'e2e-auto.${APPS_DOMAIN}:443:${IP}' 'https://e2e-auto.${APPS_DOMAIN}/' | grep -q Hostname:"
    pass "an app without a custom domain is served at e2e-auto.${APPS_DOMAIN}"
    tond apps delete e2e-auto -y > /dev/null
    ! docker network inspect tond-net-app-e2e-auto > /dev/null 2>&1 \
        || fail "deleting an app must remove its network"
    pass "deleting the app removes its network"
else
    echo "  - E2E_APPS_DOMAIN not set; skipping the automatic hostname check"
fi

step "Domains: aliases, redirects, uploaded certificates, DNS-01 wildcard"
ALIAS_DOMAIN="alias.${BASE}"
SITE_DOMAIN="site.${BASE}"
read -r code body < <(api_send POST /api/v1/apps/e2e-whoami/domains "{\"domain\": \"${ALIAS_DOMAIN}\"}")
[[ "$code" == 201 ]] || fail "adding an alias must succeed (got ${code}: ${body})"
read -r code body < <(api_send POST /api/v1/apps/e2e-whoami/domains/www-pair "{\"domain\": \"${SITE_DOMAIN}\", \"canonical\": \"apex\", \"primary\": false}")
[[ "$code" == 201 ]] || fail "adding a www/apex pair must succeed (got ${code}: ${body})"
[[ "$(app_field e2e-whoami domain)" == "$WHOAMI_DOMAIN" ]] || fail "adding aliases must keep the primary domain"
wait_for 60 "route for ${ALIAS_DOMAIN}" \
    bash -c "curl -sk --resolve '${ALIAS_DOMAIN}:443:${IP}' 'https://${ALIAS_DOMAIN}/' | grep -q Hostname:"
pass "an alias serves the app (${ALIAS_DOMAIN})"
redirect=$(curl -sk -o /dev/null -w '%{http_code} %{redirect_url}' --resolve "www.${SITE_DOMAIN}:443:${IP}" "https://www.${SITE_DOMAIN}/a/b?x=1&y=2")
[[ "$redirect" == "308 https://${SITE_DOMAIN}/a/b?x=1&y=2" ]] \
    || fail "www.${SITE_DOMAIN} must 308 to https://${SITE_DOMAIN} with path and query, got: ${redirect}"
http_redirect=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --resolve "www.${SITE_DOMAIN}:80:${IP}" "http://www.${SITE_DOMAIN}/p?q=1")
[[ "$http_redirect" == "308 https://${SITE_DOMAIN}/p?q=1" ]] \
    || fail "port 80 must send a redirect domain straight to its target, got: ${http_redirect}"
pass "the proxy answers redirect domains itself (308, path and query kept; port 80 goes straight to the target)"
read -r code body < <(api_send DELETE "/api/v1/apps/e2e-whoami/domains/${SITE_DOMAIN}" '{}')
[[ "$code" == 409 && "$(jq -r .error.code <<< "$body")" == CONFLICT && "$body" == *"redirects to"* ]] \
    || fail "removing a redirect target must be refused with CONFLICT (got ${code}: ${body})"
pass "a domain other domains redirect to can't be removed"

if command -v openssl > /dev/null; then
    UPLOAD_DOMAIN="upload.${BASE}"
    UPLOAD_DIR=$(mktemp -d)
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 30 \
        -subj "/CN=${UPLOAD_DOMAIN}" -addext "subjectAltName=DNS:${UPLOAD_DOMAIN}" \
        -keyout "${UPLOAD_DIR}/key.pem" -out "${UPLOAD_DIR}/cert.pem" 2> /dev/null
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 30 \
        -subj "/CN=other" -keyout "${UPLOAD_DIR}/other.pem" -out /dev/null 2> /dev/null
    upload_body() {
        jq -n --arg d "$UPLOAD_DOMAIN" --rawfile c "${UPLOAD_DIR}/cert.pem" --rawfile k "$1" \
            '{domain: $d, cert_pem: $c, key_pem: $k}'
    }
    read -r code body < <(api_send POST /api/v1/certs/custom "$(upload_body "${UPLOAD_DIR}/other.pem")")
    [[ "$code" == 400 && "$(jq -r '.error.code + " " + .error.field' <<< "$body")" == "INVALID_INPUT key_pem" \
        && "$body" == *"doesn't belong to the certificate"* ]] \
        || fail "an upload with the wrong key must be refused (INVALID_INPUT, field key_pem; got ${code}: ${body})"
    read -r code body < <(api_send POST /api/v1/certs/custom "$(upload_body "${UPLOAD_DIR}/key.pem")")
    [[ "$code" == 201 ]] || fail "uploading a certificate must succeed (got ${code}: ${body})"
    read -r code body < <(api_send POST /api/v1/apps/e2e-whoami/domains "{\"domain\": \"${UPLOAD_DOMAIN}\"}")
    [[ "$code" == 201 ]] || fail "adding ${UPLOAD_DOMAIN} must succeed (got ${code}: ${body})"
    served=$(openssl s_client -connect "${IP}:443" -servername "$UPLOAD_DOMAIN" < /dev/null 2> /dev/null | openssl x509 -noout -fingerprint -sha256)
    [[ "$served" == "$(openssl x509 -in "${UPLOAD_DIR}/cert.pem" -noout -fingerprint -sha256)" ]] \
        || fail "tond-proxy must serve the uploaded certificate for ${UPLOAD_DOMAIN}"
    api /api/v1/certs | jq -e --arg d "$UPLOAD_DOMAIN" '.data[] | select(.domain == $d and .cert_type == "custom" and .issuer_kind == "uploaded" and .status == "active")' > /dev/null \
        || fail "the uploaded certificate must be listed as custom/uploaded"
    [[ "$(stat -c '%a %U' "${DATA_DIR}/certs/${UPLOAD_DOMAIN}/key.pem")" == "600 tond-proxy" ]] \
        || fail "an uploaded key must be 600 tond-proxy, got: $(perms "${DATA_DIR}/certs/${UPLOAD_DOMAIN}/key.pem")"
    pass "an uploaded certificate is validated, served and stored like tond-proxy's own (key 0600)"
    read -r code body < <(api_send DELETE "/api/v1/certs/custom/${UPLOAD_DOMAIN}" '{}')
    [[ "$code" == 200 ]] || fail "removing the upload must succeed (got ${code}: ${body})"
    [[ ! -e "${DATA_DIR}/certs/${UPLOAD_DOMAIN}" ]] || fail "removing an upload must delete its files"
    pass "removing the upload deletes it (ACME takes the domain over)"
    rm -rf "$UPLOAD_DIR"
    tond apps domains remove e2e-whoami "$UPLOAD_DOMAIN" -y > /dev/null
else
    echo "  - openssl not installed; skipping the uploaded certificate check"
fi
tond apps domains remove e2e-whoami "www.${SITE_DOMAIN}" -y > /dev/null
tond apps domains remove e2e-whoami "$SITE_DOMAIN" -y > /dev/null
tond apps domains remove e2e-whoami "$ALIAS_DOMAIN" -y > /dev/null
tond apps domains list e2e-whoami --json | jq -e --arg d "$WHOAMI_DOMAIN" '.[] | select(.domain == $d and .kind == "primary")' > /dev/null \
    || fail "tond apps domains list --json must show ${WHOAMI_DOMAIN} as primary"
[[ "$(api /api/v1/apps/e2e-whoami/domains | jq '.data | length')" == 1 ]] \
    || fail "only the primary domain must be left: $(api /api/v1/apps/e2e-whoami/domains | jq -c '.data')"
pass "tond apps domains add/list/remove"

if [[ -n "$CA_FILE" && -n "$APPS_DOMAIN" ]] && curl -s -o /dev/null "${CHALLTESTSRV}/"; then
    WILDCARD="*.${APPS_DOMAIN}"
    read -r code body < <(api_send POST /api/v1/dns-providers "{\"name\": \"e2e challtestsrv\", \"kind\": \"challtestsrv\", \"zones\": [\"${APPS_DOMAIN}\"], \"token\": \"unused\", \"config\": {\"url\": \"${CHALLTESTSRV}\"}}")
    [[ "$code" == 201 ]] || fail "creating the test DNS provider must succeed (got ${code}: ${body})"
    DNS_PROVIDER_ID=$(jq -r .data.id <<< "$body")
    [[ "$body" != *'"unused"'* ]] || fail "the DNS provider token must never be returned"
    wait_for 240 "a DNS-01 wildcard certificate for ${WILDCARD}" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/certs | jq -e '.data[] | select(.domain == \"${WILDCARD}\" and .status == \"active\" and .challenge == \"dns-01\")'"
    pass "${WILDCARD} issued through DNS-01 (records published by tond-server via the provider)"
    tond apps create e2e-wild --source-type image --source traefik/whoami:v1.10 --port 80 > /dev/null
    tond apps deploy e2e-wild > /dev/null
    wait_for_app_running e2e-wild 180
    wait_for 60 "e2e-wild.${APPS_DOMAIN} served with a certificate signed by ${CA_FILE}" \
        curl -sf --cacert "$CA_FILE" --resolve "e2e-wild.${APPS_DOMAIN}:443:${IP}" "https://e2e-wild.${APPS_DOMAIN}/"
    if command -v openssl > /dev/null; then
        openssl s_client -connect "${IP}:443" -servername "e2e-wild.${APPS_DOMAIN}" < /dev/null 2> /dev/null \
            | openssl x509 -noout -ext subjectAltName | grep -qF "DNS:${WILDCARD}" \
            || fail "e2e-wild.${APPS_DOMAIN} must be served by the ${WILDCARD} certificate"
    fi
    sleep 5
    ! api /api/v1/certs | jq -e --arg d "e2e-wild.${APPS_DOMAIN}" '.data[] | select(.domain == $d)' > /dev/null \
        || fail "a host under the wildcard must not get a certificate of its own"
    pass "a new app under the apps domain is served by the wildcard at once (no per-app order)"
    tond apps delete e2e-wild -y > /dev/null
    curl -sf --unix-socket "$SOCK" -X DELETE "http://localhost/api/v1/dns-providers/${DNS_PROVIDER_ID}" > /dev/null \
        || fail "deleting the DNS provider must succeed"
else
    echo "  - no ACME CA, apps domain or challtestsrv (${CHALLTESTSRV}); skipping the DNS-01 wildcard check"
fi

step "Per-deployment images and rollback"
deployments() { api /api/v1/apps/e2e-whoami/deployments; }
FIRST_ID=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].id')
FIRST_TAG=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].image_tag')
[[ "$FIRST_TAG" =~ ^tond-e2e-whoami:[0-9a-f]{12}$ ]] || fail "deployments must run a per-deployment tag, got ${FIRST_TAG}"
docker image inspect "$FIRST_TAG" > /dev/null || fail "image ${FIRST_TAG} must exist on the host"
pass "deployment runs ${FIRST_TAG}"
# A redeploy of a serving app keeps serving the old container until the
# cutover: no failed request, and the app never leaves 'running'.
AVAIL=$(mktemp)
STATUSES=$(mktemp)
watch_availability "$WHOAMI_DOMAIN" "$AVAIL" &
AVAIL_PID=$!
( while kill -0 "$AVAIL_PID" 2> /dev/null; do app_field e2e-whoami status >> "$STATUSES" 2> /dev/null || true; sleep 1; done ) &
STATUS_PID=$!
tond apps deploy e2e-whoami > /dev/null
kill "$AVAIL_PID" "$STATUS_PID" 2> /dev/null || true
wait "$AVAIL_PID" "$STATUS_PID" 2> /dev/null || true
assert_no_downtime "$AVAIL" "redeploying e2e-whoami"
! grep -qv '^running$' "$STATUSES" \
    || fail "a serving app must stay 'running' while it redeploys, saw: $(sort -u "$STATUSES" | tr '\n' ' ')"
pass "e2e-whoami stayed 'running' during the redeploy"
wait_for 300 "a second successful e2e-whoami deployment" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e '[.data[] | select(.status == \"succeeded\")] | length >= 2'"
wait_for_app_running e2e-whoami 120
SECOND_TAG=$(deployments | jq -r '[.data[] | select(.status == "succeeded")][0].image_tag')
[[ "$SECOND_TAG" != "$FIRST_TAG" ]] || fail "each deployment must get its own tag"
deployments | jq -e --arg id "$FIRST_ID" '.data[] | select(.id == $id) | .rollback_available' > /dev/null \
    || fail "the previous deployment must be offered for rollback"
pass "redeploy ran ${SECOND_TAG}; ${FIRST_TAG} is offered for rollback"
code=$(api_req POST /api/v1/apps/e2e-whoami/rollback "{\"deployment_id\":\"${FIRST_ID}\"}")
[[ "$code" == 202 ]] || fail "rollback should be accepted, got ${code}: $(cat "$API_BODY")"
wait_for 300 "rollback to ${FIRST_TAG}" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e --arg t '${FIRST_TAG}' '.data[0] | select(.trigger_source == \"rollback\" and .status == \"succeeded\" and .image_tag == \$t)'"
wait_for_app_running e2e-whoami 120
running_image=$(docker inspect tond-e2e-whoami | jq -r '.[0].Config.Image')
[[ "$running_image" == "$FIRST_TAG" ]] || fail "rollback must run exactly ${FIRST_TAG}, got ${running_image}"
wait_for 60 "route after rollback" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
pass "rollback restored the exact previous image and traffic follows"

step "Deploy UX: phases, trigger, likely cause, DNS check"
ROLLBACK_ROW=$(deployments | jq -c '.data[0]')
[[ "$(jq -r '.rollback_of' <<< "$ROLLBACK_ROW")" == "$FIRST_ID" ]] \
    || fail "the rollback deployment must name the deployment it restored: ${ROLLBACK_ROW}"
jq -e '[.phases[] | select(.status != "skipped")] | length > 0 and all(.status == "succeeded")' <<< "$ROLLBACK_ROW" > /dev/null \
    || fail "a successful rollback must record every step as succeeded: ${ROLLBACK_ROW}"
jq -e '.duration_secs >= 0' <<< "$ROLLBACK_ROW" > /dev/null || fail "deployments must record a duration"
pass "rollback recorded its target, steps and duration"

code=$(api_req POST /api/v1/apps/e2e-whoami/deploy '{}')
[[ "$code" == 202 ]] || fail "deploy should be accepted, got ${code}: $(cat "$API_BODY")"
DEP_ID=$(jq -r '.data.deployment_id' "$API_BODY")
[[ "$DEP_ID" =~ ^[0-9a-f-]{36}$ ]] || fail "deploy must return the new deployment's id, got: $(cat "$API_BODY")"
wait_for 300 "deployment ${DEP_ID} to succeed" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments/${DEP_ID} | jq -e '.data.status == \"succeeded\"'"
DETAIL=$(api "/api/v1/apps/e2e-whoami/deployments/${DEP_ID}")
jq -e '.data.trigger_source == "cli"' <<< "$DETAIL" > /dev/null \
    || fail "a deploy over the local socket is triggered by the CLI: $(jq -c '.data | {trigger_source, trigger_detail}' <<< "$DETAIL")"
jq -e '[.data.phases[] | .phase] == ["clone","build","start","health","release"]' <<< "$DETAIL" > /dev/null \
    || fail "deployments record the five steps: $(jq -c '.data.phases' <<< "$DETAIL")"
jq -e '.data.logs | length > 0 and any(.message | test("Pulling image"))' <<< "$DETAIL" > /dev/null \
    || fail "the deployment's own log must be returned with it"
pass "deploy returned ${DEP_ID}; its steps, trigger and log are recorded"
wait_for_app_running e2e-whoami 120

# whoami listens on 80; telling Tond 8081 must fail the first deploy with
# a clear cause instead of 'running' with nothing behind the route.
tond apps create e2e-badport --source-type image --source traefik/whoami:v1.10 --port 8081 > /dev/null
code=$(api_req POST /api/v1/apps/e2e-badport/deploy '{}')
[[ "$code" == 202 ]] || fail "deploy should be accepted, got ${code}: $(cat "$API_BODY")"
BAD_ID=$(jq -r '.data.deployment_id' "$API_BODY")
wait_for 240 "deployment ${BAD_ID} to fail" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-badport/deployments/${BAD_ID} | jq -e '.data.status == \"failed\"'"
BAD=$(api "/api/v1/apps/e2e-badport/deployments/${BAD_ID}")
[[ "$(jq -r '.data.failure.code' <<< "$BAD")" == port_not_listening ]] \
    || fail "a wrong port must be classified as port_not_listening: $(jq -c '.data.failure' <<< "$BAD")"
jq -e '.data.failure.title | test("8081")' <<< "$BAD" > /dev/null || fail "the cause must name the expected port"
jq -e '.data.phases[] | select(.phase == "health") | .status == "failed"' <<< "$BAD" > /dev/null \
    || fail "the health step must be the one that failed"
[[ "$(app_field e2e-badport status)" == failed ]] || fail "a failed first deploy leaves the app failed"
[[ -z "$(docker ps -aq --filter name='^tond-e2e-badport$')" ]] || fail "the container of a failed deploy must be removed"
pass "wrong port → failed with 'port_not_listening' naming port 8081"

code=$(api_req GET "/api/v1/domains/check?domain=${WHOAMI_DOMAIN}" '')
[[ "$code" == 200 ]] || fail "domain check should answer 200, got ${code}: $(cat "$API_BODY")"
jq -e '.data.dns.status | IN("ok","mismatch","missing","inconclusive")' "$API_BODY" > /dev/null \
    || fail "domain check must report a DNS status: $(cat "$API_BODY")"
jq -e --arg d "$WHOAMI_DOMAIN" '.data.domain == $d and (.data.records | all(.type == "A" or .type == "AAAA"))' "$API_BODY" > /dev/null \
    || fail "domain check must list A/AAAA records to create: $(cat "$API_BODY")"
DNS_STATUS=$(jq -r '.data.dns.status' "$API_BODY")
code=$(api_req GET "/api/v1/domains/check?domain=not_a_domain" '')
[[ "$code" == 400 ]] || fail "an invalid domain must be rejected with 400, got ${code}"
pass "domain check reports DNS '${DNS_STATUS}' for ${WHOAMI_DOMAIN}; invalid domains → 400"

step "Build queue: coalescing, 202 + queue position, acknowledged cutover"
wait_for_app_running e2e-whoami 120
# Three deploys back to back: the first runs, the second waits, the third
# supersedes the second (one deploy in flight per app, newest wins).
q1=$(api_req POST /api/v1/apps/e2e-whoami/deploy '{}'); b1=$(cat "$API_BODY")
q2=$(api_req POST /api/v1/apps/e2e-whoami/deploy '{}'); b2=$(cat "$API_BODY")
q3=$(api_req POST /api/v1/apps/e2e-whoami/deploy '{}'); b3=$(cat "$API_BODY")
[[ "$q1 $q2 $q3" == "202 202 202" ]] || fail "deploys must be accepted with 202, got ${q1} ${q2} ${q3}"
for b in "$b1" "$b2" "$b3"; do
    echo "$b" | jq -e '.data.deployment_id and .data.status == "queued" and .data.queue_position >= 1' > /dev/null \
        || fail "a queued deploy must return its deployment id and queue position: ${b}"
done
D2=$(echo "$b2" | jq -r .data.deployment_id)
D3=$(echo "$b3" | jq -r .data.deployment_id)
[[ "$(echo "$b3" | jq -r .data.superseded)" == "$D2" ]] || fail "the third deploy must supersede the second: ${b3}"
wait_for 300 "the newest deploy to succeed" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e --arg id '${D3}' '.data[] | select(.id == \$id and .status == \"succeeded\")'"
api /api/v1/apps/e2e-whoami/deployments | jq -e --arg id "$D2" --arg by "$D3" \
    '.data[] | select(.id == $id and .status == "superseded" and .superseded_by == $by)' > /dev/null \
    || fail "the waiting deploy must be marked superseded by the newest one"
pass "3 deploys → first ran, second superseded by the third, third succeeded"
wait_for_app_running e2e-whoami 120
whoami_logs=$(tond apps logs e2e-whoami 2>/dev/null || true)
grep -q "Proxy confirmed the new route in" <<< "$whoami_logs" \
    || { tail -20 <<< "$whoami_logs" >&2; fail "the cutover must wait for tond-proxy's acknowledgement"; }
pass "cutover waited for the proxy's acknowledgement ($(tond apps logs e2e-whoami | grep -o 'confirmed the new route in [0-9]* ms' | tail -1))"
wait_for 30 "route after queued deploys" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"

step "Background worker (readiness: none — no port, no domain, never routed)"
# A worker that serves nothing must deploy (the HTTP release check would fail it).
# It lives in a project with shared env vars: the project's are the base, the
# app's own value of the same key wins.
code=$(api_req POST /api/v1/projects '{"name":"e2e-envproj","env_vars":{"E2E_SHARED":"from-project","E2E_MODE":"project"}}')
[[ "$code" == 201 ]] || fail "creating a project with shared env vars should succeed, got ${code}: $(cat "$API_BODY")"
ENVPROJ_ID=$(jq -r '.data.id' "$API_BODY")
code=$(api_req POST /api/v1/apps '{"name":"e2e-worker","source_type":"dockerfile","dockerfile_content":"FROM alpine:3\nCMD [\"sh\", \"-c\", \"while true; do echo tick; sleep 5; done\"]","readiness":"none","ready_after_secs":5,"project_id":"'"${ENVPROJ_ID}"'","env_vars":{"E2E_MODE":"app"}}')
[[ "$code" == 201 ]] || fail "creating a background worker should succeed, got ${code}: $(cat "$API_BODY")"
code=$(api_req POST /api/v1/apps '{"name":"e2e-worker-domain","source_type":"image","source_url":"alpine:3","readiness":"none","domain":"worker.example.test"}')
[[ "$code" == 400 ]] || fail "a background worker with a domain must be rejected, got ${code}"
code=$(api_req POST /api/v1/apps/e2e-worker/deploy '{}')
[[ "$code" == 202 ]] || fail "worker deploy should be accepted, got ${code}: $(cat "$API_BODY")"
WORKER_DEP=$(jq -r '.data.deployment_id' "$API_BODY")
wait_for_app_running e2e-worker 300
WORKER=$(api "/api/v1/apps/e2e-worker/deployments/${WORKER_DEP}")
jq -e '.data.status == "succeeded"' <<< "$WORKER" > /dev/null \
    || fail "the worker deployment must succeed: $(jq -c '.data | {status, failure}' <<< "$WORKER")"
jq -e '.data.phases[] | select(.phase == "health") | .label == "Stay up for 5s" and .status == "succeeded"' <<< "$WORKER" > /dev/null \
    || fail "the worker's readiness step must be 'Stay up for 5s': $(jq -c '.data.phases' <<< "$WORKER")"
[[ "$(app_field e2e-worker domain)" == null ]] || fail "a worker must not get an automatic domain"
[[ "$(app_field e2e-worker readiness)" == none ]] || fail "the app must report readiness none"
docker logs tond-e2e-worker 2>&1 | grep -q tick || fail "the worker container should be running its loop"
pass "a background worker deploys to running with no domain (readiness step: stay up 5s)"
WORKER_ENV=$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' tond-e2e-worker)
grep -qx 'E2E_SHARED=from-project' <<< "$WORKER_ENV" \
    || fail "a project's shared env var must reach its member app's container (env: ${WORKER_ENV})"
grep -qx 'E2E_MODE=app' <<< "$WORKER_ENV" \
    || fail "the app's own env var must win over the project's of the same name (env: ${WORKER_ENV})"
pass "project env vars reach member apps; the app's own value of a key wins"

step "One-click templates (catalog, generated credentials, encrypted inputs, volume owners)"
CATALOG=$(api /api/v1/templates)
jq -e '(.data.templates | length) >= 25' <<< "$CATALOG" > /dev/null \
    || fail "the template catalog should ship 25+ templates: $(jq -c '.data.templates | length' <<< "$CATALOG")"
jq -e '[.data.templates[].services[].image | select(test(":latest$") or (test(":") | not))] | length == 0' <<< "$CATALOG" > /dev/null \
    || fail "every template image must be pinned"
jq -e '.data.memory.total_mb > 0 and .data.memory.available_mb > 0' <<< "$CATALOG" > /dev/null \
    || fail "the catalog should report host memory: $(jq -c '.data.memory' <<< "$CATALOG")"
pass "catalog: $(jq '.data.templates | length' <<< "$CATALOG") templates, images pinned, free memory reported"
case "$(uname -m)" in x86_64) HOST_ARCH=amd64 ;; aarch64) HOST_ARCH=arm64 ;; *) HOST_ARCH=$(uname -m) ;; esac
[[ "$(jq -r '.data.architecture' <<< "$CATALOG")" == "$HOST_ARCH" ]] \
    || fail "the catalog should report the server's architecture (${HOST_ARCH}): $(jq -c '.data.architecture' <<< "$CATALOG")"
if [[ "$HOST_ARCH" == arm64 ]]; then
    code=$(api_req POST /api/v1/templates/mattermost/deploy '{"name":"e2e-mm","inputs":{},"allow_low_memory":true}')
    [[ "$code" == 409 ]] && jq -e '.error.message | test("ARM64")' "$API_BODY" > /dev/null \
        || fail "an x86_64-only template must be refused on ARM64, got ${code}: $(cat "$API_BODY")"
    pass "x86_64-only templates are refused on ARM64"
fi
code=$(api_req POST /api/v1/templates/vaultwarden/deploy '{"name":"Bad Name","inputs":{}}')
[[ "$code" == 400 ]] && jq -e '.error.code == "INVALID_INPUT" and .error.field == "name"' "$API_BODY" > /dev/null \
    || fail "a template deploy with an invalid name must be 400 INVALID_INPUT on field name, got ${code}: $(cat "$API_BODY")"
code=$(api_req POST /api/v1/templates/nope/deploy '{"name":"e2e-nope","inputs":{}}')
[[ "$code" == 404 ]] || fail "an unknown template must be 404, got ${code}"
# The CLI refuses options a template decides itself instead of ignoring them.
rc=0; tond apps create e2e-nope --template vaultwarden --port 8080 -y > /dev/null 2>&1 || rc=$?
[[ "$rc" == 2 ]] || fail "tond apps create --template with --port must be a usage error (exit 2), got ${rc}"
pass "tond apps create --template refuses --port and the other template-decided options (exit 2)"
code=$(api_req POST /api/v1/templates/vaultwarden/deploy \
    "{\"name\":\"e2e-vault\",\"domain\":\"${VAULT_DOMAIN}\",\"inputs\":{},\"allow_low_memory\":true}")
[[ "$code" == 202 ]] || fail "deploying the vaultwarden template should be accepted, got ${code}: $(cat "$API_BODY")"
VAULT_TOKEN=$(jq -r '.data.credentials[] | select(.key == "ADMIN_TOKEN") | .value' "$API_BODY")
[[ ${#VAULT_TOKEN} -eq 24 ]] || fail "the deploy should return the generated admin token once: $(jq -c '.data.credentials | map(.key)' "$API_BODY")"
jq -e '.data.deployment_id | length > 0' "$API_BODY" > /dev/null || fail "the deploy response needs a deployment id"
[[ "$(app_field e2e-vault template_id)" == vaultwarden ]] || fail "the app should record the template it came from"
[[ "$(app_field e2e-vault domain)" == "$VAULT_DOMAIN" ]] || fail "the template's domain should be the app's primary domain"
pass "template deploy → 202 with a generated 24-character admin token; template and domain recorded"
wait_for_app_running e2e-vault 300
VAULT_CID=$(docker ps -q --filter label=com.docker.compose.project=e2e-vault --filter label=com.docker.compose.service=vaultwarden)
[[ -n "$VAULT_CID" ]] || fail "the vaultwarden compose service should be running"
docker inspect "$VAULT_CID" --format '{{range .Config.Env}}{{println .}}{{end}}' | grep -qx "ADMIN_TOKEN=${VAULT_TOKEN}" \
    || fail "\${ADMIN_TOKEN} in the compose file should be filled from the app's env"
docker inspect "$VAULT_CID" --format '{{range .Config.Env}}{{println .}}{{end}}' | grep -qx "DOMAIN=https://${VAULT_DOMAIN}" \
    || fail "{app_url} should expand to the app's domain"
! grep -q "$VAULT_TOKEN" "${DATA_DIR}/compose/e2e-vault/docker-compose.yml" \
    || fail "the generated token must not be written into the compose file on disk"
! sqlite3 "${DATA_DIR}/tond.db" "SELECT env_vars FROM apps WHERE name = 'e2e-vault'" 2> /dev/null | grep -q "$VAULT_TOKEN" \
    || fail "the generated token must be stored encrypted"
[[ -d "${DATA_DIR}/volumes/e2e-vault/data" ]] || fail "the template's declared volume should be created"
pass "inputs reach the container through the app env, never plaintext on disk; volume created"
wait_for 90 "vaultwarden answering at https://${VAULT_DOMAIN}/alive" \
    bash -c "[[ \$(curl -sk -o /dev/null -w '%{http_code}' --resolve '${VAULT_DOMAIN}:443:${IP}' 'https://${VAULT_DOMAIN}/alive') == 200 ]]"
pass "https://${VAULT_DOMAIN}/alive → 200 through the proxy"
code=$(api_req POST /api/v1/templates/vaultwarden/deploy '{"name":"e2e-vault","inputs":{}}')
[[ "$code" == 409 ]] && jq -e '.error.code == "APP_ALREADY_EXISTS"' "$API_BODY" > /dev/null \
    || fail "a second app with the same name must be refused with APP_ALREADY_EXISTS, got ${code}: $(cat "$API_BODY")"
tond apps delete e2e-vault -y > /dev/null
code=$(api_req POST /api/v1/templates/vaultwarden/deploy '{"name":"e2e-vault","inputs":{},"allow_low_memory":true}')
[[ "$code" == 400 ]] && grep -q "earlier app named e2e-vault" "$API_BODY" \
    || fail "a template must refuse a name whose volume directory still holds data, got ${code}: $(cat "$API_BODY")"
(( $(api /api/v1/apps | jq '[.data[] | select(.name == "e2e-vault")] | length') == 0 )) \
    || fail "a refused template deploy must not leave an app behind"
rm -rf "${DATA_DIR}/volumes/e2e-vault"
pass "same name → 409; leftover data → 400 and nothing is created"

# Admin accounts: templates whose app hands the admin to its first visitor are
# pre-seeded before the domain is routed, or carry a dismissible warning.
jq -e '[.data.templates[] | select(.admin_setup == "seeded") | .id] as $s
       | [.data.templates[] | select(.admin_setup == "first_visitor") | .id] as $f
       | (["actual", "audiobookshelf", "gitea", "mattermost", "mealie", "umami"] - $s) == []
         and (["docmost", "ghost", "memos", "twenty", "uptime-kuma", "wikijs"] - $f) == []' <<< "$CATALOG" > /dev/null \
    || fail "the catalog should mark which templates pre-seed the admin and which warn: $(jq -c '[.data.templates[] | {id, admin_setup}]' <<< "$CATALOG")"
code=$(api_req POST /api/v1/templates/actual/deploy '{"name":"e2e-actual","inputs":{},"allow_low_memory":true}')
[[ "$code" == 202 ]] || fail "deploying the actual template should be accepted, got ${code}: $(cat "$API_BODY")"
ACTUAL_PW=$(jq -r '.data.credentials[] | select(.key == "SERVER_PASSWORD") | .value' "$API_BODY")
[[ ${#ACTUAL_PW} -eq 24 ]] || fail "the deploy should return the generated server password once"
wait_for_app_running e2e-actual 300
[[ "$(app_field e2e-actual admin_setup)" == done ]] \
    || fail "the server password should be set by the post_deploy step: admin_setup=$(app_field e2e-actual admin_setup) $(app_field e2e-actual admin_setup_error)"
ACTUAL_PORT=$(api /api/v1/apps/e2e-actual/services | jq -r '.data[] | select(.name == "actual") | .host_port')
curl -sf -X POST -H 'content-type: application/json' -d "$(jq -n --arg p "$ACTUAL_PW" '{password: $p}')" \
    "http://127.0.0.1:${ACTUAL_PORT}/account/login" | jq -e '.status == "ok" and (.data.token | length > 0)' > /dev/null \
    || fail "the seeded server password should sign in to Actual"
api /api/v1/apps/e2e-actual/logs | jq -e '.data.deploy_logs | any(.message | contains("Set the server password: done"))' > /dev/null \
    || fail "the deploy log should record the post_deploy step"
api /api/v1/apps/e2e-actual/logs | jq -e --arg p "$ACTUAL_PW" '[.data.deploy_logs[].message | select(contains($p))] | length == 0' > /dev/null \
    || fail "the seeded password must not appear in the deploy log"
pass "actual: the server password is set before routing (admin_setup done), signs in, stays out of the log"
code=$(api_req POST /api/v1/apps/e2e-actual/admin-setup/dismiss '{}')
[[ "$code" == 200 ]] && jq -e '.data.admin_setup == "done"' "$API_BODY" > /dev/null \
    || fail "dismissing must leave a seeded admin alone, got ${code}: $(cat "$API_BODY")"
sqlite3 "${DATA_DIR}/tond.db" "UPDATE apps SET admin_setup = 'open' WHERE name = 'e2e-actual'"
code=$(api_req POST /api/v1/apps/e2e-actual/admin-setup/dismiss '{}')
[[ "$code" == 200 ]] && jq -e '.data.admin_setup == "dismissed"' "$API_BODY" > /dev/null \
    || fail "dismissing a first-visitor warning should mark it dismissed, got ${code}: $(cat "$API_BODY")"
pass "POST /admin-setup/dismiss clears only a warning"
tond apps delete e2e-actual -y > /dev/null
rm -rf "${DATA_DIR}/volumes/e2e-actual"


step "Docker Compose app (per-service routing, status, logs, rollback, cleanup)"
# A two-service compose app: whoami gets a domain, redis stays internal with
# a healthcheck and a named volume. Tond pins each service's image per
# deployment, publishes only the routed port (on 127.0.0.1) and tond-proxy
# routes the domain to that service.
STACK_DOMAIN="stack.${BASE}"
STACK_FILE=$'services:\n  web:\n    image: traefik/whoami:v1.10\n    expose: ["80"]\n  cache:\n    image: redis:7-alpine\n    volumes: ["cache-data:/data"]\n    healthcheck:\n      test: ["CMD", "redis-cli", "ping"]\n      interval: 2s\n      timeout: 2s\n      retries: 15\nvolumes:\n  cache-data: {}\n'
stack_services() { api /api/v1/apps/e2e-stack/services; }
code=$(api_req POST /api/v1/apps "$(jq -n --arg f "$STACK_FILE" --arg d "$STACK_DOMAIN" \
    '{name: "e2e-stack", source_type: "compose", container_port: 80, domain: $d, compose_content: $f}')")
[[ "$code" == 201 ]] || fail "creating the compose app should succeed, got ${code}: $(cat "$API_BODY")"
stack_services | jq -e --arg d "$STACK_DOMAIN" \
    '[.data[] | {name, published_port, domains}] == [{name: "web", published_port: 80, domains: [$d]}, {name: "cache", published_port: null, domains: []}]' \
    > /dev/null || fail "the web service should get the port and domain: $(stack_services)"
pass "created: web publishes :80 with ${STACK_DOMAIN}, cache stays internal"
tond apps deploy e2e-stack > /dev/null
wait_for_app_running e2e-stack 300
stack_services | jq -e '[.data[] | .status] == ["running", "running"] and (.data[] | select(.name == "cache") | .health) == "healthy"' \
    > /dev/null || fail "both services should run and redis be healthy: $(stack_services)"
pass "both services running, the redis healthcheck passed"
tond --json apps services e2e-stack | jq -e '[.[].name] == ["web", "cache"]' > /dev/null \
    || fail "tond apps services should list both services: $(tond --json apps services e2e-stack)"
pass "tond apps services lists them"
wait_for 60 "route for ${STACK_DOMAIN}" bash -c "curl -sk --resolve '${STACK_DOMAIN}:443:${IP}' 'https://${STACK_DOMAIN}/' | grep -q Hostname:"
pass "https://${STACK_DOMAIN} is routed to the web service"
STACK_WEB=$(stack_services | jq -r '.data[] | select(.name == "web") | .container_id')
web_image=$(docker inspect "$STACK_WEB" | jq -r '.[0].Config.Image')
[[ "$web_image" =~ ^tond-e2e-stack/web:[0-9a-f]{12}$ ]] || fail "services must run a pinned per-deployment image, got ${web_image}"
assert_hardened "$STACK_WEB" "compose service web"
STACK_CACHE=$(stack_services | jq -r '.data[] | select(.name == "cache") | .container_id')
[[ "$(docker inspect "$STACK_CACHE" | jq -c '.[0].HostConfig.PortBindings // {}')" == "{}" ]] \
    || fail "an unpublished service must not publish ports"
pass "web runs ${web_image}, hardened; cache publishes nothing"
api /api/v1/apps/e2e-stack/services/cache/logs | jq -e '.data.lines | length > 0' > /dev/null \
    || fail "per-service logs should return redis's output"
api /api/v1/apps/e2e-stack/logs | jq -e '.data.container_logs | any(contains("[cache]"))' > /dev/null \
    || fail "the combined log should tag each line with its service"
pass "per-service and combined logs"
code=$(api_req POST /api/v1/apps/e2e-stack/services/cache/restart '{}')
[[ "$code" == 200 ]] || fail "restarting one service should work, got ${code}: $(cat "$API_BODY")"
wait_for 60 "cache running after restart" bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-stack/services | jq -e '.data[] | select(.name == \"cache\") | .status == \"running\"'"
pass "POST /services/cache/restart restarts only redis"
code=$(api_req PUT /api/v1/apps/e2e-stack/services/web "$(jq -n --arg d "$STACK_DOMAIN" '{published_port: 80, domains: [$d, "www.\($d)"]}')")
[[ "$code" == 200 ]] || fail "adding a second domain should work, got ${code}: $(cat "$API_BODY")"
wait_for 60 "route for www.${STACK_DOMAIN}" bash -c "curl -sk --resolve 'www.${STACK_DOMAIN}:443:${IP}' 'https://www.${STACK_DOMAIN}/' | grep -q Hostname:"
pass "a second domain on the same service is routed at once"
docker volume inspect e2e-stack_cache-data > /dev/null || fail "the named volume should exist"

# Redeploying an unchanged file: images are pinned by content, so compose
# keeps every container, the service port stays and nothing fails.
STACK_WEB_BEFORE=$(stack_services | jq -r '.data[] | select(.name == "web") | .container_id')
AVAIL=$(mktemp)
watch_availability "$STACK_DOMAIN" "$AVAIL" &
AVAIL_PID=$!
tond apps deploy e2e-stack > /dev/null
kill "$AVAIL_PID" 2> /dev/null || true
wait "$AVAIL_PID" 2> /dev/null || true
assert_no_downtime "$AVAIL" "redeploying e2e-stack"
[[ "$(app_field e2e-stack status)" == running ]] || fail "the compose app should still be running"
[[ "$(stack_services | jq -r '.data[] | select(.name == "web") | .container_id')" == "$STACK_WEB_BEFORE" ]] \
    || fail "an unchanged service must keep its container across a redeploy"
pass "an unchanged service keeps its container across a redeploy"

# Edit the file (web reads WHOAMI_NAME from the app's environment), redeploy,
# then roll back to the first deployment: its compose file and images come back.
STACK_FIRST=$(api /api/v1/apps/e2e-stack/deployments | jq -r '[.data[] | select(.status == "succeeded")][0].id')
STACK_FILE_V2=$'services:\n  web:\n    image: traefik/whoami:v1.10\n    expose: ["80"]\n    environment: ["WHOAMI_NAME=${STACK_NAME}"]\n  cache:\n    image: redis:7-alpine\n    volumes: ["cache-data:/data"]\n    healthcheck:\n      test: ["CMD", "redis-cli", "ping"]\n      interval: 2s\n      timeout: 2s\n      retries: 15\nvolumes:\n  cache-data: {}\n'
code=$(api_req PUT /api/v1/apps/e2e-stack/compose "$(jq -n --arg f "$STACK_FILE_V2" '{compose_content: $f}')")
[[ "$code" == 200 ]] || fail "saving the compose file should work, got ${code}: $(cat "$API_BODY")"
code=$(api_req PATCH /api/v1/apps/e2e-stack/env '{"set":{"STACK_NAME":"v2"},"unset":[]}')
[[ "$code" == 200 ]] || fail "setting an app variable should work, got ${code}: $(cat "$API_BODY")"
tond apps deploy e2e-stack > /dev/null
wait_for 300 "a third compose deployment" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-stack/deployments | jq -e '[.data[] | select(.status == \"succeeded\")] | length >= 3'"
wait_for 60 "the edited web service" bash -c "curl -sk --resolve '${STACK_DOMAIN}:443:${IP}' 'https://${STACK_DOMAIN}/' | grep -q 'Name: v2'"
pass "an edited compose file redeploys; \${STACK_NAME} came from the app's environment (Name: v2)"
api "/api/v1/apps/e2e-stack/deployments/${STACK_FIRST}" | jq -e '.data.services | length == 2 and all(.image | test("^tond-e2e-stack/"))' > /dev/null \
    || fail "a compose deployment should list its pinned service images"
code=$(api_req POST /api/v1/apps/e2e-stack/rollback "{\"deployment_id\":\"${STACK_FIRST}\"}")
[[ "$code" == 202 ]] || fail "compose rollback should be accepted, got ${code}: $(cat "$API_BODY")"
wait_for 300 "compose rollback" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-stack/deployments | jq -e '.data[0] | select(.trigger_source == \"rollback\" and .status == \"succeeded\")'"
wait_for 60 "the restored web service" bash -c "! curl -sk --resolve '${STACK_DOMAIN}:443:${IP}' 'https://${STACK_DOMAIN}/' | grep -q 'Name: v2'"
[[ "$(app_field e2e-stack compose_content)" != *WHOAMI_NAME* ]] || fail "rollback must restore the compose file too"
pass "rollback restored the first deployment's compose file and images"

tond apps delete e2e-stack -y > /dev/null
wait_for 30 "e2e-stack containers gone" bash -c "[[ -z \"\$(docker ps -aq --filter label=com.docker.compose.project=e2e-stack)\" ]]"
! docker volume inspect e2e-stack_cache-data > /dev/null 2>&1 || fail "deleting the app must remove its named volumes"
[[ -z "$(docker images -q 'tond-e2e-stack/*')" ]] || fail "deleting the app must remove its pinned images"
wait_for 30 "route for ${STACK_DOMAIN} removed" bash -c "[[ \$(curl -sk -o /dev/null -w '%{http_code}' --resolve '${STACK_DOMAIN}:443:${IP}' 'https://${STACK_DOMAIN}/') != 200 ]]"
pass "deleting the compose app removes its containers, volume, images and routes"

step "Resource metrics (cgroup sampling, restarts, cached host stats)"
# The collector samples every 30 s; the first reading only primes the rates.
wait_for 150 "e2e-whoami metrics samples" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/apps/e2e-whoami/metrics?range=1h' | jq -e '(.data.series.t | length) >= 1 and .data.current != null'"
WHOAMI_METRICS=$(api "/api/v1/apps/e2e-whoami/metrics?range=1h")
jq -e '.data.step == 30 and .data.series.mem[-1] > 0 and .data.series.pids[-1] > 0 and .data.mem_limit > 0 and (.data.series.cpu | length) == (.data.series.t | length)' \
    <<<"$WHOAMI_METRICS" > /dev/null || fail "app metrics should carry memory, pids, a limit and aligned series: ${WHOAMI_METRICS}"
pass "app samples come from cgroups (memory, pids, limit; rates computed server-side)"
wait_for 120 "e2e-redis metrics samples" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/databases/e2e-redis/metrics?range=24h' | jq -e '(.data.series.t | length) >= 1'"
pass "managed databases are measured too"
jq -e '.data.step == 7200 and (.data.series.t | length) <= 480' <<<"$(api "/api/v1/apps/e2e-whoami/metrics?range=30d")" > /dev/null \
    || fail "30d metrics should be downsampled to 2 h buckets"
[[ "$(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" 'http://localhost/api/v1/apps/e2e-whoami/metrics?range=1y')" == 400 ]] \
    || fail "an unknown metrics range must be rejected"
pass "series are downsampled per range (30d → 2 h buckets); unknown ranges → 400"

# Kill the app's process behind Docker's back: an unrequested exit that the
# restart policy recovers from must show up as a restart event.
WHOAMI_PID=$(docker inspect -f '{{.State.Pid}}' tond-e2e-whoami)
kill -9 "$WHOAMI_PID"
wait_for 90 "restart event for e2e-whoami" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/apps/e2e-whoami/metrics?range=1h' | jq -e '.data.events | any(.kind == \"restart\" and (.detail | contains(\"exit code 137\")))'"
wait_for 60 "e2e-whoami running again" bash -c "docker inspect -f '{{.State.Running}}' tond-e2e-whoami | grep -qx true"
pass "a crash-restart is recorded as a restart event (exit code 137)"

SYS_TIME=$(curl -sf -o /dev/null -w '%{time_total}' --unix-socket "$SOCK" http://localhost/api/v1/system/metrics)
awk -v t="$SYS_TIME" 'BEGIN { exit !(t < 0.15) }' || fail "/api/v1/system/metrics took ${SYS_TIME}s (it must read a cache, not sample)"
jq -e '.data.disk_total > 0 and (.data.disk_mount | length) > 0 and .data.containers_running >= 2' \
    <<<"$(api /api/v1/system/metrics)" > /dev/null || fail "system metrics should report the data directory's filesystem and Tond's containers"
pass "system metrics answer from the cache in ${SYS_TIME}s with the data directory's disk"

step "Uptime checks, heartbeat and host history"
# Probes take the public path (DNS → tond-proxy on 443 → TLS → app). The test
# domains have no public DNS and the ACME CA isn't publicly trusted, so run.sh
# gives tond-server TOND_UPTIME_RESOLVE / TOND_UPTIME_CA_FILE (tests only).
if systemctl show tond-server -p Environment | grep -q TOND_UPTIME_RESOLVE; then
    api /api/v1/system/self | jq -e '.data.loops | any(.name == "uptime")' > /dev/null \
        || fail "the uptime loop should be registered in /api/v1/system/self"
    api "/api/v1/apps/e2e-whoami/uptime" | jq -e '.data.check.interval_secs == 60 and .data.url == "https://'"${WHOAMI_DOMAIN}"'/"' > /dev/null \
        || fail "e2e-whoami should get an uptime check automatically: $(api /api/v1/apps/e2e-whoami/uptime)"
    UPTIME_CODE=$(api_req POST /api/v1/apps/e2e-whoami/uptime/check '{}')
    [[ "$UPTIME_CODE" == 200 ]] || fail "check now answered ${UPTIME_CODE}: $(cat "$API_BODY")"
    if [[ -n "$CA_FILE" ]]; then
        jq -e '.data.status == "up" and .data.last.status == 200 and .data.last.cert_days_left > 0 and .data.last.dns == "overridden"' "$API_BODY" > /dev/null \
            || fail "e2e-whoami should be up with a valid certificate (via the ACME CA): $(jq -c '.data | {status, last}' "$API_BODY")"
        pass "e2e-whoami is up through DNS/TLS/proxy (HTTP 200, certificate valid for $(jq -r .data.last.cert_days_left "$API_BODY") more days)"
    else
        jq -e '.data.last != null' "$API_BODY" > /dev/null || fail "check now should record a probe"
        pass "check now probed e2e-whoami (no ACME CA: TLS isn't verified here)"
    fi
    # Stop the container behind Tond's back: the proxy answers 502 → down (http_5xx).
    docker stop tond-e2e-whoami > /dev/null
    [[ "$(api_req POST /api/v1/apps/e2e-whoami/uptime/check '{}')" == 200 ]] || fail "check now failed: $(cat "$API_BODY")"
    jq -e '.data.status == "down" and .data.incidents[0].cause == "http_5xx" and .data.incidents[0].ended_at == null' "$API_BODY" > /dev/null \
        || fail "a stopped container should open an http_5xx incident: $(jq -c '.data | {status, last, incidents}' "$API_BODY")"
    pass "a dead app is reported down with the cause (HTTP 502 from the proxy) and an open incident"
    docker start tond-e2e-whoami > /dev/null
    wait_for 60 "e2e-whoami back behind the proxy" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
    if [[ -n "$CA_FILE" ]]; then
        [[ "$(api_req POST /api/v1/apps/e2e-whoami/uptime/check '{}')" == 200 ]] || fail "check now failed: $(cat "$API_BODY")"
        jq -e '.data.status == "up" and .data.incidents[0].ended_at != null' "$API_BODY" > /dev/null \
            || fail "recovery should close the incident: $(jq -c '.data | {status, incidents}' "$API_BODY")"
        pass "recovery closes the incident"
    fi
    UPTIME_RUNS=$(api /api/v1/system/self | jq '.data.loops[] | select(.name == "uptime") | .runs')
    wait_for 150 "a scheduled uptime round" \
        bash -c "(( \$(curl -sf --unix-socket '$SOCK' http://localhost/api/v1/system/self | jq '.data.loops[] | select(.name == \"uptime\") | .runs') > ${UPTIME_RUNS} ))"
    api /api/v1/uptime | jq -e '.data[] | select(.name == "e2e-whoami") | .uptime.day_probes >= 2 and .uptime.day != null' > /dev/null \
        || fail "GET /api/v1/uptime should report e2e-whoami's probes: $(api /api/v1/uptime)"
    pass "the uptime loop probes on its own on the minute grid (GET /api/v1/uptime)"
    api /api/v1/system/uptime | jq -e '.data.check.path == "/health"' > /dev/null || fail "the dashboard should have its own check"
    pass "the dashboard has its own uptime check (/health)"
    [[ "$(api_req PUT /api/v1/apps/e2e-whoami/uptime '{"enabled":true,"path":"nope","interval_secs":60,"expected_status":null,"timeout_secs":10}')" == 400 ]] \
        || fail "an invalid check path must be rejected"
    pass "check settings are validated"
else
    echo "  - TOND_UPTIME_RESOLVE not set for tond-server (not run via e2e/run.sh); skipping probe checks"
fi

# Heartbeat: a local listener stands in for healthchecks.io.
HB_PORT=18477
HB_LOG=$(mktemp)
python3 - "$HB_PORT" "$HB_LOG" <<'PY' &
import http.server, sys
port, log = int(sys.argv[1]), sys.argv[2]
class Hit(http.server.BaseHTTPRequestHandler):
    def hit(self):
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path}\n")
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()
    do_GET = hit
    do_POST = hit
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", port), Hit).serve_forever()
PY
HB_PID=$!
wait_for 10 "the heartbeat listener" bash -c "curl -sf http://127.0.0.1:${HB_PORT}/ready"
[[ "$(api_req PUT /api/v1/system/heartbeat '{"url":"http://127.0.0.1:'"${HB_PORT}"'/ping/e2e-hb-secret","method":"POST","interval_secs":60,"enabled":true}')" == 200 ]] \
    || fail "saving the heartbeat failed: $(cat "$API_BODY")"
jq -e '.data.configured and .data.url_host == "127.0.0.1"' "$API_BODY" > /dev/null || fail "heartbeat settings not saved: $(cat "$API_BODY")"
if grep -q e2e-hb-secret "$API_BODY" || api /api/v1/system/heartbeat | grep -q e2e-hb-secret; then
    fail "the heartbeat URL must be write-only"
fi
[[ "$(api_req POST /api/v1/system/heartbeat/test '{}')" == 200 ]] || fail "test ping failed: $(cat "$API_BODY")"
jq -e '.data.outcome == "sent"' "$API_BODY" > /dev/null || fail "the test ping should be sent: $(cat "$API_BODY")"
grep -q "POST /ping/e2e-hb-secret" "$HB_LOG" || fail "the listener didn't receive the test ping"
pass "heartbeat URL is write-only; a test ping reaches the monitor"
wait_for 150 "a scheduled heartbeat" bash -c "[[ \$(grep -c 'POST /ping/e2e-hb-secret' '$HB_LOG') -ge 2 ]]"
api /api/v1/system/heartbeat | jq -e '.data.last_ok_at != null and .data.healthy' > /dev/null \
    || fail "the heartbeat should be healthy with a recent success: $(api /api/v1/system/heartbeat)"
pass "the scheduled heartbeat is sent while Tond is healthy (Docker, proxy, loops)"
[[ "$(api_req PUT /api/v1/system/heartbeat '{"url":""}')" == 200 ]] || fail "removing the heartbeat URL failed"
kill "$HB_PID" 2> /dev/null || true
grep -q "e2e-hb-secret" /opt/tond/data/tond.db && fail "the heartbeat URL must be encrypted at rest"
pass "the heartbeat URL is encrypted at rest and can be removed"

# Host history: one row a minute from the metrics tick.
wait_for 150 "host history samples" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/system/host/history?range=1h' | jq -e '(.data.series.t | length) >= 1'"
jq -e '.data.step == 60 and .data.series.disk_total[-1] > 0 and .data.series.inodes_total[-1] >= .data.series.inodes_used[-1] and .data.series.mem_total[-1] > 0 and (.data.series.load1 | length) == (.data.series.t | length)' \
    <<<"$(api '/api/v1/system/host/history?range=1h')" > /dev/null || fail "host history should carry disk, inodes, memory and load: $(api '/api/v1/system/host/history?range=1h' | head -c 600)"
# (btrfs reports no inode counts: inodes_total is 0 there.)
pass "host history is recorded every minute (CPU, load, memory, swap, disk, inodes)"
api /api/v1/system/host/forecast | jq -e '.data.day.status | IN("filling", "stable", "insufficient_data")' > /dev/null \
    || fail "the disk forecast should answer: $(api /api/v1/system/host/forecast)"
HOST=$(api /api/v1/system/host)
jq -e '.data.docker.reachable and (.data.docker.version | length) > 0 and (.data.docker.storage_driver | length) > 0 and .data.current.data_fs.total > 0' \
    <<<"$HOST" > /dev/null || fail "host overview should report Docker's health and the filesystems: ${HOST}"
pass "disk forecast answers; Docker is reachable ($(jq -r '.data.docker.version + ", " + .data.docker.storage_driver' <<<"$HOST"))"
DISK=$(curl -sf --unix-socket "$SOCK" -X POST http://localhost/api/v1/system/disk/refresh)
jq -e '.data.error == null and .data.images.count >= 1 and .data.images.bytes > 0 and .data.tond_data > 0' <<<"$DISK" > /dev/null \
    || fail "the disk breakdown should list Docker images and the data directory: ${DISK}"
PREVIEW_CLEANUP=$(api /api/v1/system/disk/cleanup)
jq -e '(.data.total_bytes | type) == "number" and (.data.dangling_images | type) == "array"' <<<"$PREVIEW_CLEANUP" > /dev/null \
    || fail "the cleanup preview should list what would be removed: ${PREVIEW_CLEANUP}"
[[ "$(api_req POST /api/v1/system/disk/cleanup '{}')" == 200 ]] || fail "free up space failed: $(cat "$API_BODY")"
api "/api/v1/audit-log?action=disk_cleanup&limit=1" | jq -e '.data.entries | length > 0' > /dev/null \
    || fail "free up space must be audited"
proxied_body "$WHOAMI_DOMAIN" / | grep -q Hostname: || fail "free up space must not touch running apps"
pass "disk breakdown (images, build cache, data dir); free up space removes only build cache and dangling images, audited"

step "Dashboard delivery (precompressed, cached)"
INDEX_HEADERS=$(curl -sk -D - -o /dev/null -H 'Accept-Encoding: br, gzip' --resolve "${DASH}:443:${IP}" "https://${DASH}/")
grep -qi '^content-encoding: br' <<<"$INDEX_HEADERS" || fail "index.html should be served brotli-compressed: ${INDEX_HEADERS}"
grep -qi '^cache-control: no-cache' <<<"$INDEX_HEADERS" || fail "index.html must revalidate (no-cache)"
ASSET=$(curl -sk --resolve "${DASH}:443:${IP}" "https://${DASH}/" | grep -o '/_app/immutable/[^"]*\.js' | head -1)
[[ -n "$ASSET" ]] || fail "no hashed asset referenced from index.html"
ASSET_HEADERS=$(curl -sk -D - -o /dev/null -H 'Accept-Encoding: br, gzip' --resolve "${DASH}:443:${IP}" "https://${DASH}${ASSET}")
grep -qi '^cache-control: public, max-age=31536000, immutable' <<<"$ASSET_HEADERS" || fail "hashed assets must be immutable: ${ASSET_HEADERS}"
grep -qi '^content-encoding: br' <<<"$ASSET_HEADERS" || fail "hashed assets should be brotli-compressed"
[[ "$(proxied_status "$DASH" /_app/immutable/chunks/missing-deadbeef.js)" == 404 ]] \
    || fail "a missing build file must be 404, not the SPA shell"
[[ -z "$(find /opt/tond/dashboard -name '*.ttf')" ]] || fail "the dashboard should ship no TTF fonts"
pass "index.html br + no-cache; hashed assets br + immutable; missing chunks 404; no TTF fonts"

step "Operations: certificates, retention, app events, deploy trend, /metrics token, audit shipping, deploy --follow"
certs=$(api /api/v1/certs)
jq -e --arg d "$WHOAMI_DOMAIN" '.data[] | select(.domain == $d)
    | .serves.kind == "app" and .serves.app_name == "e2e-whoami" and (.issuer_kind | type == "string")' \
    <<<"$certs" > /dev/null || fail "GET /api/v1/certs must say what each certificate serves: $(jq -c . <<<"$certs")"
pass "the certificate list names what each domain serves"
if [[ -n "$CA_FILE" ]]; then
    jq -e --arg d "$WHOAMI_DOMAIN" '.data[] | select(.domain == $d)
        | .issuer != null and .last_renewed_at != null and (.expires_at | test("^[0-9]{4}-"))' <<<"$certs" > /dev/null \
        || fail "an issued certificate must record its issuer, last renewal and an RFC 3339 expiry"
    pass "issued certificates record issuer and last renewal"
fi

retention=$(api /api/v1/settings/retention)
jq -e '([.data.fields[] | select(.key == "retention_audit_log_days")][0].default == 180)
    and .data.database.file_bytes > 0 and (.data.database.tables | map(.name) | index("audit_log"))' \
    <<<"$retention" > /dev/null || fail "retention settings must carry defaults and the database size per table"
read -r code _ < <(api_send PUT /api/v1/settings/retention '{"retention_audit_log_days": 5}')
[[ "$code" == 400 ]] || fail "retention values outside their bounds must be rejected (got ${code})"
pass "retention settings: roadmap defaults, bounds enforced, database size per table"
if [[ -f /etc/systemd/journald.conf.d/tond.conf ]]; then
    grep -q '^SystemMaxUse=500M' /etc/systemd/journald.conf.d/tond.conf || fail "the journald drop-in must cap the journal at 500M"
    pass "the journal is capped at 500 MB"
else
    grep -rqsE '^[[:space:]]*SystemMaxUse' /etc/systemd/journald.conf /etc/systemd/journald.conf.d/ \
        || fail "install.sh must cap the journal (or keep an existing SystemMaxUse)"
    pass "an existing journal cap was kept"
fi

events=$(api "/api/v1/apps/e2e-whoami/events?limit=200")
jq -e '[.data.events[].kind] | index("deploy_succeeded") and index("restart")' <<<"$events" > /dev/null \
    || fail "the app event feed must show deploys and the restart: $(jq -c '[.data.events[].kind]' <<<"$events")"
pass "app events merge deploys and restarts ($(jq '.data.events | length' <<<"$events") events)"
stats=$(api /api/v1/apps/e2e-whoami/deployments/stats)
jq -e '.data.window >= 2 and .data.success_rate != null and .data.median_duration_secs != null' <<<"$stats" > /dev/null \
    || fail "deploy stats must report a success rate and median duration: $(jq -c .data <<<"$stats")"
pass "deploy trend: $(jq '.data.success_rate * 100 | floor' <<<"$stats")% of the last $(jq .data.window <<<"$stats") deploys succeeded"

tond token revoke e2e-prometheus -y > /dev/null 2>&1 || true
PROM_TOKEN=$(tond token create --name e2e-prometheus --scope metrics:read --expires 1d 2> /dev/null)
prom=$(curl -sf -H "Authorization: Bearer ${PROM_TOKEN}" http://127.0.0.1:3001/metrics) \
    || fail "/metrics over TCP must accept a metrics:read token"
grep -q '^tond_process_resident_memory_bytes' <<<"$prom" || fail "/metrics over TCP must expose Prometheus metrics"
[[ "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${RO_TOKEN}" http://127.0.0.1:3001/api/v1/settings/retention)" == 200 ]] \
    || fail "any signed-in caller may read retention settings"
pass "Prometheus can scrape /metrics over TCP with a metrics:read token"

if command -v python3 > /dev/null; then
    SYSLOG_OUT=$(mktemp)
    python3 -c '
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 16514)); s.listen(5)
out = open(sys.argv[1], "ab")
while True:
    c, _ = s.accept()
    while True:
        d = c.recv(65536)
        if not d: break
        out.write(d); out.flush()
    c.close()' "$SYSLOG_OUT" &
    SYSLOG_PID=$!
    sleep 1
    read -r code body < <(api_send PUT /api/v1/audit-log/shipping '{"syslog": {"enabled": true, "host": "127.0.0.1", "port": 16514, "tls": false}}')
    [[ "$code" == 200 ]] || fail "enabling syslog shipping should succeed (got ${code}: ${body})"
    wait_for 60 "audit entries shipped to the syslog receiver" bash -c "grep -q 'audit_shipping_update' '$SYSLOG_OUT'"
    wait_for 60 "syslog shipping caught up" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/audit-log/shipping | jq -e '.data.syslog.status.pending == 0 and .data.syslog.status.last_error == null'"
    grep -qE '^[0-9]+ <1(08|09)>1 ' "$SYSLOG_OUT" || fail "shipped entries must be octet-counted RFC 5424 messages"
    grep -q '"hash":"' "$SYSLOG_OUT" || fail "shipped entries must carry their chain hash"
    read -r code _ < <(api_send PUT /api/v1/audit-log/shipping '{"syslog": {"enabled": false, "host": "127.0.0.1", "port": 16514, "tls": false}}')
    kill "$SYSLOG_PID" 2> /dev/null || true
    rm -f "$SYSLOG_OUT"
    pass "audit entries (with hashes) are shipped to syslog as RFC 5424 and delivery is tracked"
else
    echo "  - python3 not available; skipping the audit shipping check"
fi

follow_out=$(tond apps deploy e2e-whoami 2>&1) || fail "tond apps deploy must exit 0 for a good deploy: ${follow_out}"
grep -q "is live" <<<"$follow_out" || fail "tond apps deploy must follow the deploy to the end: ${follow_out}"
grep -q "✓ " <<<"$follow_out" || fail "tond apps deploy must print phase progress: ${follow_out}"
queued_out=$(tond apps deploy e2e-whoami --no-follow 2>&1) || fail "tond apps deploy --no-follow failed: ${queued_out}"
grep -q "queued" <<<"$queued_out" || fail "--no-follow must return once the deploy is queued: ${queued_out}"
wait_for 300 "the --no-follow deploy to finish" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e '[.data[] | select(.status == \"running\" or .status == \"queued\")] | length == 0'"
wait_for_app_running e2e-whoami 120
pass "tond apps deploy follows phases to the end; --no-follow returns at once"

step "Snapshot"
tond snapshot create --name e2e-smoke > /dev/null
wait_for 120 "e2e-smoke snapshot completed" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/snapshots | jq -e '.data[] | select(.name == \"e2e-smoke\" and .status == \"completed\")'"
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
# never left 'running'.
api_req POST /api/v1/apps/e2e-whoami/deploy '{}' > /dev/null
INTERRUPTED=$(jq -r .data.deployment_id "$API_BODY")
systemctl kill -s SIGKILL tond-server
wait_for 60 "API health after a crash" curl -sf http://127.0.0.1:3001/health
status=$(api /api/v1/apps/e2e-whoami/deployments | jq -r --arg id "$INTERRUPTED" '.data[] | select(.id == $id) | .status')
case "$status" in
    failed)
        api /api/v1/apps/e2e-whoami/deployments | jq -e --arg id "$INTERRUPTED" '.data[] | select(.id == $id) | .failure.code == "interrupted"' > /dev/null \
            || fail "an interrupted deploy must say so"
        pass "a deploy cut off by a crash is marked failed (interrupted) at startup" ;;
    queued|running|succeeded)
        # It had not started yet (resumed from the queue) or finished first.
        wait_for 300 "the resumed deploy to finish" \
            bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e --arg id '${INTERRUPTED}' '.data[] | select(.id == \$id and (.status == \"succeeded\" or .status == \"failed\"))'"
        pass "a deploy caught by the crash was resumed or had finished (status was ${status})" ;;
    *) fail "unexpected status for the interrupted deploy: '${status}'" ;;
esac
unfinished=$(api /api/v1/apps/e2e-whoami/deployments | jq '[.data[] | select(.status == "running")] | length')
wait_for 300 "no deployment left in flight" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/apps/e2e-whoami/deployments | jq -e '[.data[] | select(.status == \"running\" or .status == \"queued\")] | length == 0'"
wait_for_app_running e2e-whoami 120
pass "startup reconciliation left nothing in flight (had ${unfinished} right after the crash)"

step "Stability"
running_runs=$(api "/api/v1/cron/${CRON_ID}/runs" | jq '[.data[] | select(.status == "running")] | length')
[[ "$running_runs" == 0 ]] || fail "no cron run may stay 'running' after a restart (found ${running_runs})"
pass "no cron run left 'running' after the restart"
[[ "$(api "/api/v1/cron/${CRON_ID}" | jq -r '.data.status')" != running ]] \
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

step "Self-update: version check, update runs, automatic rollback (System → Updates)"
UPDATE_STATUS="${DATA_DIR}/update/status.json"
overview=$(api /api/v1/system/update)
CURRENT_VERSION=$(jq -r .data.current_version <<<"$overview")
[[ -n "$CURRENT_VERSION" && "$CURRENT_VERSION" != null ]] || fail "GET /system/update must report the running version"
jq -e '.data.schema_version >= 1' <<<"$overview" > /dev/null || fail "GET /system/update must report the schema version"
jq -e '.data.unsupported_reason == null' <<<"$overview" > /dev/null \
    || fail "updates must be possible from the dashboard here: $(jq -r .data.unsupported_reason <<<"$overview")"
pass "GET /system/update reports Tond ${CURRENT_VERSION} and that it can update itself"
# The release check, against a local releases file instead of GitHub.
cat > "${DATA_DIR}/e2e-releases.json" <<'JSON'
[{"tag_name":"v99.0.0","name":"Tond 99.0.0","body":"## Highlights\n- e2e release notes","html_url":"https://github.com/tahayvr/tond/releases/tag/v99.0.0","published_at":"2026-09-20T10:00:00Z","draft":false,"prerelease":false},
 {"tag_name":"v99.1.0-rc.1","name":"rc","body":"","html_url":"x","published_at":null,"draft":false,"prerelease":true}]
JSON
printf '[Service]\nEnvironment=TOND_UPDATE_RELEASES_URL=file://%s/e2e-releases.json\n' "$DATA_DIR" \
    > /etc/systemd/system/tond-server.service.d/e2e-releases.conf
systemctl daemon-reload
systemctl restart tond-server
wait_for 30 "API health after restart" curl -sf http://127.0.0.1:3001/health
[[ "$(api_req POST /api/v1/system/update/check '{}')" == 200 ]] || fail "POST /system/update/check failed: $(cat "$API_BODY")"
jq -e '.data.update_available and .data.latest.version == "99.0.0" and (.data.changelog | length == 1) and (.data.changelog[0].body | contains("e2e release notes"))' "$API_BODY" > /dev/null \
    || fail "the release check must find 99.0.0 (and skip the pre-release): $(cat "$API_BODY")"
pass "the release check finds a newer release with its notes and skips pre-releases"
[[ "$(api_req PUT /api/v1/system/update/settings '{"check_enabled":false}')" == 200 ]] || fail "turning the daily check off failed"
jq -e '.data.check.enabled == false' "$API_BODY" > /dev/null || fail "the daily check must be off"
api_req PUT /api/v1/system/update/settings '{"check_enabled":true}' > /dev/null
pass "the daily update check can be turned off and on"
code=$(api_req POST /api/v1/system/update/rollback '{}')
[[ "$code" == 400 ]] && jq -e '.error.message | test("7 days")' "$API_BODY" > /dev/null \
    || fail "rollback without a previous version must be refused (${code}): $(cat "$API_BODY")"
pass "no rollback is offered before an update"
code=$(api_req POST /api/v1/system/update '{"version":"0.0.1"}')
[[ "$code" == 400 ]] || fail "installing an older release without force must be refused (${code})"
pass "an older release is refused without force"

# An update that can't be downloaded: snapshot, transient unit, failure,
# nothing changed.
[[ "$(api_req POST /api/v1/system/update '{"version":"99.0.0"}')" == 202 ]] || fail "POST /system/update failed: $(cat "$API_BODY")"
RUN_ID=$(jq -r .data.id "$API_BODY")
code=$(api_req POST /api/v1/system/update '{"version":"99.0.0"}')
[[ "$code" == 409 ]] || fail "a second update while one runs must be refused with 409 (got ${code})"
wait_for 300 "the 99.0.0 update run to finish" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/system/update | jq -e --arg id '$RUN_ID' '.data.run.id == \$id and .data.run.effective_state != \"running\"'"
run=$(api /api/v1/system/update | jq -c .data.run)
[[ "$(jq -r .effective_state <<<"$run")" == failed ]] || fail "an update that can't be downloaded must fail: $run"
jq -e '.message | test("Nothing was changed")' <<<"$run" > /dev/null || fail "the failure must say nothing changed: $run"
jq -e '.snapshot_id != null' <<<"$run" > /dev/null || fail "the run must record its pre-update snapshot: $run"
api /api/v1/snapshots | jq -e '.data[] | select(.trigger == "pre_update" and .status == "completed")' > /dev/null \
    || fail "a completed pre_update snapshot must exist"
[[ "$(api /api/v1/system/update | jq -r .data.current_version)" == "$CURRENT_VERSION" ]] || fail "the running version must not change"
systemctl is-active --quiet tond-update.service && fail "tond-update.service must be gone after the run"
wait_for 30 "the update's audit entries" bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/v1/audit-log?action=tond_update&limit=1' | jq -e '.data.entries[0].detail | test(\"99.0.0\")'"
api '/api/v1/audit-log?action=tond_update_start&limit=1' | jq -e '.data.entries | length == 1' > /dev/null \
    || fail "starting an update must be audited"
pass "an update that can't be downloaded: pre-update snapshot, tond-update.service, failed with nothing changed, audited"

if [[ -n "${E2E_BUNDLE:-}" && -d "${E2E_BUNDLE}" ]]; then
    # update.sh with a local bundle, as the unit runs it after verifying a download.
    TOND_UPDATE_HEALTH_TIMEOUT=60 /opt/tond/bin/update.sh --bundle "$E2E_BUNDLE" --force \
        --run-id e2e-reinstall --actor e2e > /tmp/e2e-update.log 2>&1 \
        || { cat /tmp/e2e-update.log >&2; fail "update.sh --bundle --force must succeed"; }
    jq -e '.state == "succeeded"' "$UPDATE_STATUS" > /dev/null || fail "the reinstall must end succeeded: $(cat "$UPDATE_STATUS")"
    wait_for 30 "API health after the reinstall" curl -sf http://127.0.0.1:3001/health
    systemctl is-active --quiet tond-proxy || fail "tond-proxy must run after the reinstall"
    wait_for 60 "route after the reinstall" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
    backup=$(find "${DATA_DIR}/backups" -maxdepth 1 -name 'tond_backup_*' | sort | tail -n 1)
    [[ -f "${backup}/tond.db" && -f "${backup}/bin/tond-server" ]] || fail "the update must back up the database and binaries (${backup})"
    grep -q '^MIGRATED=0$' "${backup}/info.env" || fail "reinstalling the same version must record no migration: $(cat "${backup}/info.env")"
    [[ "$(perms "${DATA_DIR}/update")" == "700 root root" ]] || fail "the update directory must be root-only ($(perms "${DATA_DIR}/update"))"
    pass "update.sh reinstalls from a bundle: backup with the database, services and routes back, status succeeded"

    # A "new version" whose tond-server never comes up: the health check fails
    # and the previous version comes back on its own.
    NEXT="$(mktemp -d)/bundle"
    cp -a "$E2E_BUNDLE" "$NEXT"
    printf '#!/bin/sh\necho "e2e: this tond-server never starts" >&2\nexit 1\n' > "${NEXT}/tond-server"
    chmod +x "${NEXT}/tond-server"
    before_db=$(sha256sum "${DATA_DIR}/tond.db" | awk '{print $1}')
    rc=0
    TOND_UPDATE_HEALTH_TIMEOUT=20 /opt/tond/bin/update.sh --bundle "$NEXT" --force \
        --run-id e2e-rollback --actor e2e > /tmp/e2e-update.log 2>&1 || rc=$?
    [[ "$rc" == 1 ]] || { cat /tmp/e2e-update.log >&2; fail "an unhealthy update must exit 1 after rolling back (got ${rc})"; }
    jq -e '.state == "rolled_back" and (.message | test("did not become healthy"))' "$UPDATE_STATUS" > /dev/null \
        || fail "the update must be rolled back: $(cat "$UPDATE_STATUS")"
    wait_for 30 "API health after the rollback" curl -sf http://127.0.0.1:3001/health
    [[ "$(api /api/v1/system/update | jq -r .data.current_version)" == "$CURRENT_VERSION" ]] || fail "the previous version must run again"
    [[ "$(tr -d 'v[:space:]' < /opt/tond/VERSION)" == "$(tr -d 'v[:space:]' < "${E2E_BUNDLE}/VERSION")" ]] || fail "VERSION must be restored"
    systemctl is-active --quiet tond-proxy || fail "tond-proxy must run after the rollback"
    wait_for 60 "route after the rollback" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
    [[ "$(app_field e2e-whoami status)" == running ]] || fail "apps must keep running through a failed update"
    after_db=$(sha256sum "${DATA_DIR}/tond.db" | awk '{print $1}')
    jq -e '.database_restored == true' "$UPDATE_STATUS" > /dev/null \
        || fail "a server that never started may have half-migrated: the database copy must come back"
    [[ -n "$before_db" && -n "$after_db" ]] || fail "could not hash tond.db"
    cmp -s /opt/tond/bin/tond-server "${E2E_BUNDLE}/tond-server" || fail "the working tond-server binary must be back"
    pass "an update whose server never starts rolls back by itself: binaries and database back, services and routes up, status rolled_back"
    rm -rf "$(dirname "$NEXT")"
fi
rm -f /etc/systemd/system/tond-server.service.d/e2e-releases.conf "${DATA_DIR}/e2e-releases.json"
systemctl daemon-reload
systemctl restart tond-server
wait_for 30 "API health after restart" curl -sf http://127.0.0.1:3001/health

if [[ "$CLEANUP" == 1 ]]; then
    step "Cleanup"
    delete_e2e_resources
    pass "e2e resources deleted"
fi

echo -e "\n${GREEN}${BOLD}All smoke tests passed.${NC}"
