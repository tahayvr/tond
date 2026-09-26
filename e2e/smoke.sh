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
callback=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' \
    "${API_TCP}/api/github-apps/callback?code=e2eattackercode")
[[ "$callback" == "303 "*"error=missing_state"* ]] \
    || fail "the GitHub App callback without a pending state must be refused (got: ${callback})"
callback=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' \
    "${API_TCP}/api/github-apps/callback?code=e2eattackercode&state=forged")
[[ "$callback" == "303 "*"error=not_logged_in"* ]] \
    || fail "the GitHub App callback without an admin session must be refused (got: ${callback})"
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
