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
#   E2E_FIXTURE_DIR       static site pushed via `tond apps push`
#                         (default: fixtures/static-site next to this script)
#   E2E_CLEANUP           1 to delete the created resources at the end (default: 0)
#   E2E_ADMIN_EMAIL       admin account the auth checks create (via the setup token)
#   E2E_ADMIN_PASSWORD    or sign in with on re-runs (defaults below)

BASE="${E2E_BASE_DOMAIN:?E2E_BASE_DOMAIN is required}"
DASH="${E2E_DASHBOARD_DOMAIN:-dash.${BASE}}"
PREVIEW="${E2E_PREVIEW_DOMAIN:-}"
APPS_DOMAIN="${E2E_APPS_DOMAIN:-}"
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
    for app in e2e-whoami e2e-static e2e-rooted e2e-compose e2e-auto; do
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
    [[ -e "${DATA_DIR}/${f}" ]] || continue
    [[ "$(perms "${DATA_DIR}/${f}")" == "660 root tond-proxy" ]] \
        || fail "${f} must be 660 root:tond-proxy, got: $(perms "${DATA_DIR}/${f}")"
done
pass "tond.db (+ -wal/-shm) is 660 root:tond-proxy"
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

# Automation gets scoped tokens from the root CLI socket; the secret is printed once.
for name in e2e-readonly e2e-admin e2e-ci e2e-env e2e-revoked e2e-expired e2e-ip-ok e2e-ip-other; do
    tond token revoke "$name" > /dev/null 2>&1 || true # re-runs on a long-lived VM
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
    code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' -d '{"enabled":true}' http://127.0.0.1:3001/api/settings/host-terminal)
    [[ "$code" == 403 ]] || fail "an API token must not be able to enable the host terminal, got ${code}"
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
    code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' -d "$ticket_body" http://127.0.0.1:3001/api/terminal/tickets)
    [[ "$code" == 403 ]] || fail "API tokens must not get terminal tickets, even with terminal:host, got ${code}"
    pass "API tokens are refused terminal tickets"
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

step "Users, roles and the password policy"
# Invite over the root socket (users:admin), set the password through the
# one-time link, act as a viewer, then disable and delete the account.
VIEWER_EMAIL="e2e-viewer@tond.test"
old_id=$(api /api/users | jq -r --arg e "$VIEWER_EMAIL" '.data[] | select(.email == $e) | .id')
[[ -z "$old_id" ]] || api_req DELETE "/api/users/${old_id}" '{}' > /dev/null
code=$(api_req POST /api/users "{\"email\":\"${VIEWER_EMAIL}\",\"role\":\"viewer\"}")
[[ "$code" == 201 ]] || fail "inviting a user must succeed, got ${code}: $(cat "$API_BODY")"
VIEWER_ID=$(jq -r '.data.user.id' "$API_BODY")
LINK_TOKEN=$(jq -r '.data.token' "$API_BODY")
[[ "$(jq -r '.data.user.password_set' "$API_BODY")" == false ]] || fail "invited users have no password until they use the link"
pass "invite creates a passwordless user and a one-time set-password link"
VIEWER_COOKIES="$(mktemp)"
link_complete() {
    curl -sk --resolve "${DASH}:443:${IP}" -b "$VIEWER_COOKIES" -c "$VIEWER_COOKIES" -o "$API_BODY" -w '%{http_code}' \
        -X POST -H "Origin: https://${DASH}" -H 'X-Tond-CSRF: 1' -H 'Content-Type: application/json' \
        -d "{\"token\":\"${LINK_TOKEN}\",\"password\":\"$1\"}" "https://${DASH}/api/auth/password-link/complete"
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
[[ "$(viewer_code GET /api/apps)" == 200 ]] || fail "viewers can read apps"
code=$(viewer_code PUT /api/settings '{"server_name":"viewer-was-here"}')
[[ "$code" == 403 && "$(jq -r '.code' "$API_BODY")" == SCOPE_DENIED ]] || fail "viewers must get 403 SCOPE_DENIED for settings, got ${code}"
[[ "$(viewer_code GET /api/users)" == 403 ]] || fail "viewers can't list users"
pass "a viewer can read but gets 403 SCOPE_DENIED for writes"
code=$(api_req PATCH "/api/users/${VIEWER_ID}" '{"disabled":true}')
[[ "$code" == 200 ]] || fail "disabling a user must succeed, got ${code}"
[[ "$(viewer_code GET /api/apps)" == 401 ]] || fail "disabling a user must end their sessions"
pass "disabling a user signs them out everywhere"
[[ "$(api_req DELETE "/api/users/${VIEWER_ID}" '{}')" == 200 ]] || fail "deleting a user must succeed"
api "/api/audit-log?resource_type=user&resource_id=${VIEWER_ID}" \
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

step "Webhook and integration hardening"
API_TCP="http://127.0.0.1:3001"
# status_of <curl args...>: HTTP status code of a request to the TCP API (no credentials).
status_of() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
WHOAMI_ID=$(app_field e2e-whoami id)
[[ "$WHOAMI_ID" =~ ^[0-9a-f-]{36}$ ]] || fail "could not read the e2e-whoami app id (got: ${WHOAMI_ID})"
WEBHOOK_JSON=$(api "/api/apps/${WHOAMI_ID}/webhook")
[[ "$(jq -r '.data.has_secret' <<< "$WEBHOOK_JSON")" == true && "$(jq -r '.data.secret // empty' <<< "$WEBHOOK_JSON")" == "" ]] \
    || fail "the webhook config must say a secret is set without returning it"
WEBHOOK_SECRET=$(api "/api/apps/${WHOAMI_ID}/webhook-secret" | jq -r '.data.secret // empty')
[[ ${#WEBHOOK_SECRET} -ge 32 ]] || fail "every app must get a generated webhook secret"
pass "app has a generated webhook secret (write-only; revealed on request)"
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

step "API token scopes, filters and audit"
# api_as <token> <METHOD> <path> [json]: HTTP status over loopback TCP; body in $API_BODY.
api_as() {
    curl -s -o "$API_BODY" -w '%{http_code}' -X "$2" -H "Authorization: Bearer $1" \
        -H 'Content-Type: application/json' ${4:+-d "$4"} "http://127.0.0.1:3001$3"
}
body_code() { jq -r '.code // empty' "$API_BODY"; }
CI_TOKEN=$(tond token create --name e2e-ci --kind ci --preset deployer --app e2e-whoami --expires 1d 2> /dev/null)
[[ "$(api_as "$CI_TOKEN" GET /api/apps/e2e-whoami)" == 200 ]] || fail "the CI token must read its own app"
api_as "$CI_TOKEN" GET /api/apps > /dev/null
[[ "$(jq -r '.data | map(.name) | join(",")' "$API_BODY")" == e2e-whoami ]] \
    || fail "a token limited to e2e-whoami must only list e2e-whoami, got: $(cat "$API_BODY")"
pass "a resource-limited token sees only its app"
code=$(api_as "$CI_TOKEN" GET /api/apps/e2e-static)
[[ "$code" == 403 && "$(body_code)" == RESOURCE_DENIED ]] \
    || fail "the CI token must be refused other apps with 403 RESOURCE_DENIED, got ${code} $(cat "$API_BODY")"
pass "resource filter blocks other apps (403 RESOURCE_DENIED)"
code=$(api_as "$RO_TOKEN" PUT /api/settings '{"server_name":"hijacked"}')
[[ "$code" == 403 && "$(body_code)" == SCOPE_DENIED ]] \
    || fail "a read-only token must get 403 SCOPE_DENIED for settings, got ${code} $(cat "$API_BODY")"
code=$(api_as "$CI_TOKEN" DELETE /api/apps/e2e-whoami)
[[ "$code" == 403 && "$(body_code)" == SCOPE_DENIED ]] \
    || fail "a deployer token must get 403 SCOPE_DENIED for apps:delete, got ${code}"
pass "missing scopes → 403 with the stable code SCOPE_DENIED"

REVOKED=$(tond token create --name e2e-revoked --preset readonly --expires 1d 2> /dev/null)
[[ "$(api_as "$REVOKED" GET /api/apps)" == 200 ]] || fail "a fresh token must work"
tond token revoke e2e-revoked > /dev/null
code=$(api_as "$REVOKED" GET /api/apps)
[[ "$code" == 401 && "$(body_code)" == TOKEN_REVOKED ]] || fail "a revoked token must get 401 TOKEN_REVOKED, got ${code}"
pass "revoked token → 401 TOKEN_REVOKED"
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
    code=$(api_as "$EXPIRED" GET /api/apps)
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
[[ "$(api_as "$IP_OK" GET /api/apps)" == 200 ]] || fail "a token allowed from 127.0.0.1 must work over loopback"
code=$(api_as "$IP_OTHER" GET /api/apps)
[[ "$code" == 403 && "$(body_code)" == IP_NOT_ALLOWED ]] || fail "a token pinned to another IP must get 403 IP_NOT_ALLOWED, got ${code}"
code=$(curl -sk -o /dev/null -w '%{http_code}' --resolve "${DASH}:443:${IP}" -H "Authorization: Bearer ${IP_OTHER}" \
    -H 'X-Forwarded-For: 10.255.255.1' "https://${DASH}/api/apps")
[[ "$code" == 403 ]] || fail "a spoofed X-Forwarded-For through the proxy must not satisfy the allow-list, got ${code}"
pass "allowed-IP filter works and can't be spoofed through the proxy"

ENV_TOKEN=$(tond token create --name e2e-env --scope env:write --app e2e-whoami --expires 1d 2> /dev/null)
[[ "$(api_as "$ENV_TOKEN" PATCH /api/apps/e2e-whoami/env '{"set":{"E2E_TOKEN_AUDIT":"1"},"unset":[]}')" == 200 ]] \
    || fail "an env:write token must patch env vars: $(cat "$API_BODY")"
entry=$(api '/api/audit-log?actor=e2e-env&action=env_update&limit=1' | jq -c '.data.entries[0]')
[[ "$(jq -r '.actor_kind' <<< "$entry")" == token && "$(jq -r '.actor_id' <<< "$entry")" != null ]] \
    || fail "token calls must be audited with the token's id and name as actor, got: ${entry}"
api_as "$ENV_TOKEN" PATCH /api/apps/e2e-whoami/env '{"set":{},"unset":["E2E_TOKEN_AUDIT"]}' > /dev/null
[[ "$(api '/api/audit-log?action=token_create&actor=local-socket&limit=1' | jq -r '.data.entries[0].actor_kind')" == local_socket ]] \
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
REDIS_CREDS=$(api /api/databases/e2e-redis/credentials)
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
[[ "$(api_req PATCH "/api/apps/${WHOAMI_ID}/env" "{\"set\":{\"E2E_CANARY\":\"${CANARY}\"}}")" == 200 ]] \
    || fail "setting an env var must succeed: $(cat "$API_BODY")"
grep -qF "$CANARY" "$API_BODY" && fail "the env patch response must not echo the value"
api "/api/apps/${WHOAMI_ID}/env" | grep -qF "$CANARY" && fail "the env listing must mask values"
[[ "$(api "/api/apps/${WHOAMI_ID}/env/E2E_CANARY" | jq -r '.data.value')" == "$CANARY" ]] \
    || fail "revealing an env var must return its value"
for f in "${DATA_DIR}"/tond.db*; do
    grep -qaF "$CANARY" "$f" && fail "env var value found in plaintext in ${f}"
    grep -qaF "$REDIS_PASS" "$f" && fail "database password found in plaintext in ${f}"
    grep -qaF "$WEBHOOK_SECRET" "$f" && fail "webhook secret found in plaintext in ${f}"
done
pass "env values, database passwords and webhook secrets are not stored in plaintext"

for action in env_reveal webhook_secret_reveal db_credentials_reveal; do
    api "/api/audit-log?action=${action}&limit=1" | jq -e '.data.entries | length > 0' > /dev/null \
        || fail "a ${action} must be audit-logged"
done
pass "every secret reveal is audit-logged"

step "Per-database backup"
[[ "$(api_req POST /api/databases/e2e-redis/backups '{}')" == 201 ]] \
    || fail "backing up a database must succeed: $(cat "$API_BODY")"
[[ "$(jq -r '.data.status + " " + .data.scope' "$API_BODY")" == "completed database:e2e-redis" ]] \
    || fail "a database backup must complete with the database scope: $(cat "$API_BODY")"
DB_BACKUP_ID=$(jq -r '.data.id' "$API_BODY")
api /api/databases/e2e-redis/backups | jq -e --arg id "$DB_BACKUP_ID" '.data[] | select(.id == $id)' > /dev/null \
    || fail "the database backup must be listed for the database"
[[ -d "${DATA_DIR}/backups/snapshots/${DB_BACKUP_ID}/databases/e2e-redis" ]] \
    || fail "the database backup must hold the database's data"
pass "database backed up and listed"
[[ "$(api_req POST "/api/snapshots/${DB_BACKUP_ID}/restore-database" '{"db_name":"e2e-redis"}')" == 200 ]] \
    || fail "restoring the database must succeed: $(cat "$API_BODY")"
SAFETY_ID=$(jq -r '.data.pre_restore_snapshot_id // empty' "$API_BODY")
api /api/snapshots | jq -e --arg id "$SAFETY_ID" '.data[] | select(.id == $id and .scope == "database:e2e-redis" and .trigger == "pre_restore")' > /dev/null \
    || fail "a database restore must back up the current data first"
wait_for 60 "e2e-redis answering after the restore" \
    bash -c "docker exec -e REDISCLI_AUTH='${REDIS_PASS}' tond-db-e2e-redis redis-cli --no-auth-warning --user '${REDIS_USER}' PING | grep -qx PONG"
pass "database restored from its backup (safety-net backup taken first)"

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

read -r code body < <(api_send PUT /api/databases/e2e-redis/links/e2e-whoami '{}')
[[ "$code" == 200 ]] && jq -e '.data.linked_apps | any(.name == "e2e-whoami")' <<<"$body" > /dev/null \
    || fail "linking e2e-whoami to e2e-redis must succeed (got ${code}: ${body})"
wait_for 20 "e2e-redis reachable by name from e2e-whoami's network" redis_reachable "$WHOAMI_NET" e2e-redis
pass "after linking, e2e-whoami reaches e2e-redis by name"
api /api/apps/e2e-whoami/databases | jq -e '.data | any(.name == "e2e-redis" and .via == "link")' > /dev/null \
    || fail "GET /api/apps/e2e-whoami/databases must list the linked database"
read -r code body < <(api_send PUT /api/databases/e2e-redis/links/e2e-static '{}')
[[ "$code" == 200 ]] || fail "linking e2e-static must succeed (got ${code}: ${body})"
! http_ok "$STATIC_NET" "$WHOAMI_IP" \
    || fail "two apps linked to the same database must still not reach each other"
pass "apps linked to the same database still cannot reach each other"
read -r code body < <(api_send DELETE /api/databases/e2e-redis/links/e2e-static '{}')
[[ "$code" == 200 ]] || fail "unlinking e2e-static must succeed (got ${code}: ${body})"
read -r code body < <(api_send DELETE /api/databases/e2e-redis/links/e2e-whoami '{}')
[[ "$code" == 200 ]] || fail "unlinking e2e-whoami must succeed (got ${code}: ${body})"
wait_for 30 "e2e-redis unreachable from e2e-whoami after unlinking" \
    redis_unreachable "$WHOAMI_NET" "$REDIS_IP"
[[ "$(networks_of tond-db-e2e-redis)" == "${REDIS_NET} " ]] \
    || fail "unlinking must detach e2e-redis from the apps' networks (on: $(networks_of tond-db-e2e-redis))"
pass "unlinking detaches the database again"
code=$(api_req POST /api/apps '{"name":"e2e-compose","source_type":"compose","container_port":80,"compose_content":"services:\n  web:\n    image: nginx:alpine\nnetworks:\n  shared:\n    external: true\n    name: tond-net-db-e2e-redis\n"}')
[[ "$code" == 400 ]] && grep -q "external" "$API_BODY" \
    || fail "a compose file joining another network must be rejected, got ${code}: $(cat "$API_BODY")"
pass "compose files cannot join another app's or database's network"

step "Upload safety (archives are extracted in Rust, with limits)"
UPLOAD_TMP=$(mktemp -d)
ln -s /etc/shadow "${UPLOAD_TMP}/shadow"
tar -czf "${UPLOAD_TMP}/evil.tar.gz" -C "$UPLOAD_TMP" shadow
code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" \
    -F "file=@${UPLOAD_TMP}/evil.tar.gz" http://localhost/api/apps/e2e-static/upload)
[[ "$code" == 400 ]] && grep -q "outside the project" "$API_BODY" \
    || fail "an archive with a symlink to /etc/shadow must be refused with 400, got ${code}: $(cat "$API_BODY")"
pass "an archive linking outside the project is refused (400, says why)"
read -r code body < <(api_send PUT /api/settings '{"upload_max_bytes": 1048576}')
[[ "$code" == 200 ]] || fail "setting a 1 MiB upload limit must succeed (got ${code}: ${body})"
head -c 3145728 /dev/zero > "${UPLOAD_TMP}/zeros.bin"
tar -czf "${UPLOAD_TMP}/bomb.tar.gz" -C "$UPLOAD_TMP" zeros.bin
code=$(curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" \
    -F "file=@${UPLOAD_TMP}/bomb.tar.gz" http://localhost/api/apps/e2e-static/upload)
read -r restore_code _ < <(api_send PUT /api/settings '{"upload_max_bytes": 1073741824}')
[[ "$restore_code" == 200 ]] || fail "restoring the upload limit must succeed"
[[ "$code" == 413 ]] && grep -q "tondignore" "$API_BODY" \
    || fail "an archive that unpacks past the limit must be refused with 413, got ${code}: $(cat "$API_BODY")"
pass "an archive that unpacks past upload_max_bytes is refused (413, says how to fix it)"
[[ -z "$(ls -A "${DATA_DIR}/uploads" 2> /dev/null)" ]] || fail "refused uploads must not leave files in ${DATA_DIR}/uploads"
pass "refused uploads leave nothing behind"
rm -rf "$UPLOAD_TMP"
[[ "$(app_field e2e-static status)" == running ]] || fail "refused uploads must not touch the running app"

step "Apps domain (automatic hostnames never on the dashboard's site)"
api /api/apps/e2e-whoami | jq -e '.data.domain_warning | strings | contains("same site as your dashboard")' > /dev/null \
    || fail "a custom domain on the dashboard's site (${WHOAMI_DOMAIN}) must carry a warning"
pass "custom domain ${WHOAMI_DOMAIN} (same site as ${DASH}) is allowed with a warning"
read -r code body < <(api_send PUT /api/settings "{\"apps_domain\": \"apps.${BASE}\"}")
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

step "Resource metrics (cgroup sampling, restarts, cached host stats)"
# The collector samples every 30 s; the first reading only primes the rates.
wait_for 150 "e2e-whoami metrics samples" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/apps/e2e-whoami/metrics?range=1h' | jq -e '(.data.series.t | length) >= 1 and .data.current != null'"
WHOAMI_METRICS=$(api "/api/apps/e2e-whoami/metrics?range=1h")
jq -e '.data.step == 30 and .data.series.mem[-1] > 0 and .data.series.pids[-1] > 0 and .data.mem_limit > 0 and (.data.series.cpu | length) == (.data.series.t | length)' \
    <<<"$WHOAMI_METRICS" > /dev/null || fail "app metrics should carry memory, pids, a limit and aligned series: ${WHOAMI_METRICS}"
pass "app samples come from cgroups (memory, pids, limit; rates computed server-side)"
wait_for 120 "e2e-redis metrics samples" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/databases/e2e-redis/metrics?range=24h' | jq -e '(.data.series.t | length) >= 1'"
pass "managed databases are measured too"
jq -e '.data.step == 7200 and (.data.series.t | length) <= 480' <<<"$(api "/api/apps/e2e-whoami/metrics?range=30d")" > /dev/null \
    || fail "30d metrics should be downsampled to 2 h buckets"
[[ "$(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$SOCK" 'http://localhost/api/apps/e2e-whoami/metrics?range=1y')" == 400 ]] \
    || fail "an unknown metrics range must be rejected"
pass "series are downsampled per range (30d → 2 h buckets); unknown ranges → 400"

# Kill the app's process behind Docker's back: an unrequested exit that the
# restart policy recovers from must show up as a restart event.
WHOAMI_PID=$(docker inspect -f '{{.State.Pid}}' tond-e2e-whoami)
kill -9 "$WHOAMI_PID"
wait_for 90 "restart event for e2e-whoami" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/apps/e2e-whoami/metrics?range=1h' | jq -e '.data.events | any(.kind == \"restart\" and (.detail | contains(\"exit code 137\")))'"
wait_for 60 "e2e-whoami running again" bash -c "docker inspect -f '{{.State.Running}}' tond-e2e-whoami | grep -qx true"
pass "a crash-restart is recorded as a restart event (exit code 137)"

SYS_TIME=$(curl -sf -o /dev/null -w '%{time_total}' --unix-socket "$SOCK" http://localhost/api/system/metrics)
awk -v t="$SYS_TIME" 'BEGIN { exit !(t < 0.15) }' || fail "/api/system/metrics took ${SYS_TIME}s (it must read a cache, not sample)"
jq -e '.data.disk_total > 0 and (.data.disk_mount | length) > 0 and .data.containers_running >= 2' \
    <<<"$(api /api/system/metrics)" > /dev/null || fail "system metrics should report the data directory's filesystem and Tond's containers"
pass "system metrics answer from the cache in ${SYS_TIME}s with the data directory's disk"

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
