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

BASE="${E2E_BASE_DOMAIN:?E2E_BASE_DOMAIN is required}"
DASH="${E2E_DASHBOARD_DOMAIN:-dash.${BASE}}"
PREVIEW="${E2E_PREVIEW_DOMAIN:-}"
IP="${E2E_TARGET_IP:-127.0.0.1}"
CA_FILE="${E2E_CA_FILE:-}"
FIXTURE_DIR="${E2E_FIXTURE_DIR:-$(cd "$(dirname "$0")" && pwd)/fixtures/static-site}"
CLEANUP="${E2E_CLEANUP:-0}"
SOCK="/opt/tond/data/tond.sock"
DATA=/opt/tond/data

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
    for app in e2e-whoami e2e-static; do tond apps delete "$app" -y > /dev/null 2>&1 || true; done
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
[[ "$(perms "$DATA")" == "3770 root tond-proxy" ]] \
    || fail "${DATA} must be 3770 root:tond-proxy, got: $(perms "$DATA")"
pass "${DATA} is 3770 root:tond-proxy (setgid + sticky, no access for others)"
for f in tond.db tond.db-wal tond.db-shm; do
    [[ -e "${DATA}/${f}" ]] || continue
    [[ "$(perms "${DATA}/${f}")" == "660 root tond-proxy" ]] \
        || fail "${f} must be 660 root:tond-proxy, got: $(perms "${DATA}/${f}")"
done
pass "tond.db (+ -wal/-shm) is 660 root:tond-proxy"
world=$(find "$DATA" -maxdepth 1 -mindepth 1 -perm /o+rwx)
[[ -z "$world" ]] || fail "top-level entries accessible to others: ${world}"
for d in keys backups volumes databases builds compose docker-config; do
    [[ ! -e "${DATA}/${d}" || "$(perms "${DATA}/${d}")" == "700 root root" ]] \
        || fail "${DATA}/${d} must be 700 root:root, got: $(perms "${DATA}/${d}")"
done
pass "no top-level entry is accessible to others; private subdirectories are 0700 root"
if runuser -u nobody -- cat "${DATA}/tond.db" > /dev/null 2>&1; then
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
if runuser -u tond-proxy -- cat "$SECRET_ENV" > /dev/null 2>&1; then
    fail "tond-proxy must not be able to read tond.env"
fi
if runuser -u tond-proxy -- test -w /var/run/docker.sock; then
    fail "tond-proxy must not have access to the Docker socket"
fi
if runuser -u tond-proxy -- rm -f "$SECRET_ENV" 2> /dev/null || [[ ! -f "$SECRET_ENV" ]]; then
    fail "tond-proxy must not be able to remove root's files from the data directory"
fi
pass "tond-proxy cannot read or remove tond.env, or reach the Docker socket"
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

ROOT_KEY=$(journalctl -u tond-server --no-pager | grep -A1 "ROOT API KEY" | grep -oE '[0-9a-f]{64}' | tail -1 || true)
if [[ -n "$ROOT_KEY" ]]; then
    tond --server http://127.0.0.1:3001 --api-key "$ROOT_KEY" apps list > /dev/null \
        || fail "root API key must work over TCP"
    pass "root API key accepted over TCP"
    if tond --server http://127.0.0.1:3001 --api-key "$(printf '0%.0s' {1..64})" apps list > /dev/null 2>&1; then
        fail "a wrong API key must be rejected"
    fi
    pass "wrong API key rejected"
else
    echo "  - root API key not found in the journal (reused install?); skipping key checks"
fi

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
else
    echo "  - E2E_CA_FILE not set; skipping certificate verification"
fi
# Issued (or self-signed fallback) certificates and the ACME account belong to
# tond-proxy and are owner-only.
bad=$(find "${DATA}/certs" \( -type f ! -perm 600 \) -o \( -type d ! -perm 700 \) -o ! -user tond-proxy)
[[ -z "$bad" ]] || fail "certs must be tond-proxy-owned 0600 files / 0700 dirs: ${bad}"
if [[ -e "${DATA}/acme_account.json" ]]; then
    [[ "$(stat -c '%a %U' "${DATA}/acme_account.json")" == "600 tond-proxy" ]] \
        || fail "acme_account.json must be 600 tond-proxy, got: $(perms "${DATA}/acme_account.json")"
fi
pass "TLS keys and the ACME account key are 0600, owned by tond-proxy"

step "Preview domain isolation"
if [[ -n "$PREVIEW" ]]; then
    tond settings | grep -q "preview-domain *${PREVIEW}" || fail "install.sh must set the preview domain to ${PREVIEW}"
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

step "Cron job"
tond cron create e2e-cron --schedule "0 0 1 1 *" --image alpine:3 --command "echo tond-e2e-cron-ok" > /dev/null
CRON_ID=$(api /api/cron | jq -r '.data[] | select(.name == "e2e-cron") | .id')
tond cron trigger e2e-cron > /dev/null
wait_for 180 "a successful e2e-cron run" \
    bash -c "curl -sf --unix-socket '$SOCK' 'http://localhost/api/cron/${CRON_ID}/runs' | jq -e '.data[] | select(.status == \"success\" and (.output // \"\" | contains(\"tond-e2e-cron-ok\")))'"
pass "manual cron run succeeded with the expected output"

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

if [[ "$CLEANUP" == 1 ]]; then
    step "Cleanup"
    delete_e2e_resources
    pass "e2e resources deleted"
fi

echo -e "\n${GREEN}${BOLD}All smoke tests passed.${NC}"
