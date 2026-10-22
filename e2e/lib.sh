#!/usr/bin/env bash
# Shared helpers for the end-to-end scripts (smoke.sh, release smoke). Source
# it after setting SOCK (tond-server's Unix socket) and IP (what the test
# domains resolve to).

SOCK="${SOCK:-/opt/tond/data/tond.sock}"
IP="${IP:-127.0.0.1}"

# ── Output ────────────────────────────────────────────────────────────────────

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
step() { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }
pass() { echo -e "  ${GREEN}✓${NC} $*"; }
fail() { echo -e "  ${RED}✗ $*${NC}" >&2; exit 1; }

# ── Requests ──────────────────────────────────────────────────────────────────

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

# ── Apps ──────────────────────────────────────────────────────────────────────

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

# ── Self-update ───────────────────────────────────────────────────────────────

# wait_for_update_run <run-id> <timeout-seconds>: until the update/rollback run
# <run-id> (GET /api/v1/system/update) is no longer running; prints its JSON.
wait_for_update_run() {
    wait_for "$2" "the update run $1 to finish" \
        bash -c "curl -sf --unix-socket '$SOCK' http://localhost/api/v1/system/update | jq -e --arg id '$1' '.data.run.id == \$id and .data.run.effective_state != \"running\"'"
    api /api/v1/system/update | jq -c .data.run
}

# current_version: the version the running tond-server reports.
current_version() { api /api/v1/system/update | jq -r .data.current_version; }

# db_sql <sql>: run one statement against tond.db with python3 (installed on
# every supported host, unlike the sqlite3 CLI) and print the rows, one per
# line, columns separated by |. Writes are committed.
db_sql() {
    python3 -c 'import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
for row in c.execute(sys.argv[2]):
    print("|".join("" if v is None else str(v) for v in row))
c.commit()' "${DATA_DIR}/tond.db" "$1"
}
