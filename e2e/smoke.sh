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
#   E2E_TARGET_IP         IP that the domains resolve to (default: 127.0.0.1)
#   E2E_CA_FILE           root CA of the ACME server (Pebble / LE staging). When set,
#                         the test waits for a real certificate and verifies it
#                         against this CA; when unset the TLS issuance check is skipped.
#   E2E_FIXTURE_DIR       static site pushed via `tond apps push`
#                         (default: fixtures/static-site next to this script)
#   E2E_CLEANUP           1 to delete the created resources at the end (default: 0)

BASE="${E2E_BASE_DOMAIN:?E2E_BASE_DOMAIN is required}"
DASH="${E2E_DASHBOARD_DOMAIN:-dash.${BASE}}"
IP="${E2E_TARGET_IP:-127.0.0.1}"
CA_FILE="${E2E_CA_FILE:-}"
FIXTURE_DIR="${E2E_FIXTURE_DIR:-$(cd "$(dirname "$0")" && pwd)/fixtures/static-site}"
CLEANUP="${E2E_CLEANUP:-0}"
SOCK="/opt/tond/data/tond.sock"

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

# api_send <METHOD> <path> <json>: send JSON over the socket, print the HTTP
# status; the response body lands in $API_BODY.
API_BODY=$(mktemp)
api_send() {
    curl -s -o "$API_BODY" -w '%{http_code}' --unix-socket "$SOCK" -X "$1" \
        -H 'Content-Type: application/json' -d "$3" "http://localhost$2"
}

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
assert_hardened "$(app_field e2e-whoami container_id)" "app e2e-whoami"
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

code=$(api_send POST /api/apps '{"name":"e2e-rooted","source_type":"image","source_url":"alpine:3","container_port":80,"volumes":[{"container_path":"/host","host_path":"/"}]}')
[[ "$code" == 400 ]] && grep -q "is not allowed" "$API_BODY" \
    || fail "a host_path of / must be rejected with 400, got ${code}: $(cat "$API_BODY")"
[[ -z "$(app_field e2e-rooted id)" ]] || fail "no app may be created for a rejected volume"
pass "POST /api/apps with host_path \"/\" → 400, nothing created"
code=$(api_send POST /api/apps/e2e-whoami/volumes '{"container_path":"/sock","host_path":"/var/run/docker.sock"}')
[[ "$code" == 400 ]] || fail "binding docker.sock into an app must be rejected, got ${code}: $(cat "$API_BODY")"
pass "POST /api/apps/{id}/volumes with docker.sock → 400"
code=$(api_send POST /api/apps '{"name":"e2e-compose","source_type":"compose","container_port":80,"compose_content":"services:\n  web:\n    image: nginx:alpine\n    privileged: true\n    network_mode: host\n"}')
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
code=$(api_send POST /api/apps/e2e-whoami/rollback "{\"deployment_id\":\"${FIRST_ID}\"}")
[[ "$code" == 202 ]] || fail "rollback should be accepted, got ${code}: $(cat "$API_BODY")"
wait_for 300 "rollback to ${FIRST_TAG}" \
    bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/apps/e2e-whoami/deployments | jq -e --arg t '${FIRST_TAG}' '.data[0] | select(.trigger_source == \"rollback\" and .status == \"succeeded\" and .image_tag == \$t)'"
wait_for_app_running e2e-whoami 120
running_image=$(docker inspect "$(app_field e2e-whoami container_id)" | jq -r '.[0].Config.Image')
[[ "$running_image" == "$FIRST_TAG" ]] || fail "rollback must run exactly ${FIRST_TAG}, got ${running_image}"
wait_for 60 "route after rollback" bash -c "curl -sk --resolve '${WHOAMI_DOMAIN}:443:${IP}' 'https://${WHOAMI_DOMAIN}/' | grep -q Hostname:"
pass "rollback restored the exact previous image and traffic follows"

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
