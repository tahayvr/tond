#!/usr/bin/env bash
set -euo pipefail

# Tond resource budget test.
#
# Runs as root ON a host with a fresh Tond install (like smoke.sh; `just
# vm-budget` wipes the local VM, installs and runs this). It measures an idle
# Tond with 0 apps, then with N dummy apps, and fails when a budget is
# exceeded:
#
#   budget                                   0 apps          N apps
#   RSS tond-server / tond-proxy             ≤ 15 / 12 MB    ≤ 40 / 25 MB
#   CPU, both processes together             ≤ 0.2 % of one core (idle)
#   background wakeups (loops) server/proxy  ≤ 6 / 2 per minute
#   SQL statements run by tond-server        ≤ 10 per minute, none a full table scan
#   journal lines per service                ≤ 10 per hour
#   log WebSocket soak (open/close ×200)     Tokio task count back to baseline
#
# Wakeups are the background loops' own wakeups as reported by
# GET /api/system/self (server) and the proxy's control-socket stats. The OS
# context-switch rate is printed too, for information: it includes Tokio's
# and Pingora's internal timers, which Tond does not control.
#
# Dummy apps are rows written straight into tond.db: status 'running', a
# domain and a host port, no container. The proxy routes them (so its table
# holds N routes) and the server's loops see them, without running N
# containers.
#
# Usage: e2e/budget.sh [--window <seconds>] [--apps <n>] [--no-assert] [--json <file>]
#   --window     measurement window per phase (default 180)
#   --apps       dummy apps for the second phase (default 50; 0 skips it)
#   --no-assert  report only (e.g. to measure an older build)
#   --json       also write the measurements as JSON

WINDOW=180
APPS=50
ASSERT=1
JSON_OUT=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --window) WINDOW="$2"; shift 2 ;;
        --apps) APPS="$2"; shift 2 ;;
        --no-assert) ASSERT=0; shift ;;
        --json) JSON_OUT="$2"; shift 2 ;;
        -h|--help) sed -n '4,36p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

SOCK="/opt/tond/data/tond.sock"
DB="/opt/tond/data/tond.db"
MB=$((1024 * 1024))

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
step() { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }
info() { echo -e "  $*"; }
FAILURES=()
RESULTS="{}"

# check <label> <value> <op> <limit> <unit>: record and print one budget.
check() {
    local label=$1 value=$2 op=$3 limit=$4 unit=$5 ok
    ok=$(python3 -c "import sys; v, l = float(sys.argv[1]), float(sys.argv[2]); print(1 if (v <= l if sys.argv[3] == '<=' else v >= l) else 0)" "$value" "$limit" "$op")
    RESULTS=$(jq -c --arg k "$label" --argjson v "$value" --argjson l "$limit" '. + {($k): {value: $v, limit: $l}}' <<<"$RESULTS")
    if [[ "$ok" == 1 ]]; then
        echo -e "  ${GREEN}✓${NC} ${label}: ${value} ${unit} (budget ${op} ${limit})"
    else
        echo -e "  ${RED}✗ ${label}: ${value} ${unit} (budget ${op} ${limit})${NC}"
        FAILURES+=("${label}: ${value} ${unit} > ${limit}")
    fi
}

api() { curl -sf --unix-socket "$SOCK" "http://localhost$1"; }
pid_of() { systemctl show -p MainPID --value "$1"; }
# Resident memory in bytes, then the part that is Rosetta's. Under Rosetta
# (x86_64 binaries in an Apple Silicon VM) the translator keeps its code cache
# in anonymous mappings at the top of the address space inside the process;
# those pages are not Tond's and don't exist on a real x86_64 host, so they
# are subtracted (and reported).
rss_bytes() {
    python3 - "$1" <<'PY'
import re, sys
pid = sys.argv[1]
total = rosetta = 0
under_rosetta = False
start, name = 0, ""
for line in open(f"/proc/{pid}/smaps"):
    m = re.match(r"^([0-9a-f]+)-[0-9a-f]+ \S+ \S+ \S+ \S+\s*(.*)$", line)
    if m:
        start, name = int(m.group(1), 16), m.group(2)
        if "rosetta" in name:
            under_rosetta = True
        continue
    if line.startswith("Rss:"):
        kb = int(line.split()[1])
        total += kb
        if "rosetta" in name or start >= 0xE000_0000_0000:
            rosetta += kb
if not under_rosetta:
    rosetta = 0
print((total - rosetta) * 1024, rosetta * 1024)
PY
}
# utime + stime in seconds (fields 14, 15 after the command name).
cpu_seconds() { awk '{ sub(/^.*\) /, ""); print ($12 + $13) / 100 }' "/proc/$1/stat"; }
ctx_switches() {
    local total=0 f
    for f in /proc/"$1"/task/*/status; do
        total=$((total + $(awk '/^(non)?voluntary_ctxt_switches:/ { s += $2 } END { print s + 0 }' "$f")))
    done
    echo "$total"
}
journal_lines() { journalctl -u "$1" --since "@$2" --until "@$3" --no-pager -o cat 2>/dev/null | wc -l | tr -d ' '; }
calc() { python3 -c "print(round($1, 3))"; }

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }
for tool in curl jq python3 journalctl; do command -v "$tool" > /dev/null || { echo "missing tool: $tool" >&2; exit 1; }; done
api /api/system/self > /dev/null || { echo "tond-server does not answer GET /api/system/self on ${SOCK}" >&2; exit 1; }

# Let startup work finish (the janitor's first pass runs 2 minutes after start).
settle() {
    local up
    up=$(api /api/system/self | jq -r .data.server.uptime_secs)
    if (( up < 150 )); then
        info "waiting $((150 - up))s for startup work to finish (uptime ${up}s)"
        sleep $((150 - up))
    fi
    # And for the proxy to have loaded the current routes.
    sleep 5
}

# sample <file>: snapshot of everything we measure.
sample() {
    local server proxy
    server=$(pid_of tond-server)
    proxy=$(pid_of tond-proxy)
    api /api/system/self | jq -c \
        --argjson t "$(date +%s.%N)" \
        --argjson srss "$(rss_bytes "$server" | cut -d' ' -f1)" --argjson prss "$(rss_bytes "$proxy" | cut -d' ' -f1)" \
        --argjson srosetta "$(rss_bytes "$server" | cut -d' ' -f2)" --argjson prosetta "$(rss_bytes "$proxy" | cut -d' ' -f2)" \
        --argjson scpu "$(cpu_seconds "$server")" --argjson pcpu "$(cpu_seconds "$proxy")" \
        --argjson sctx "$(ctx_switches "$server")" --argjson pctx "$(ctx_switches "$proxy")" \
        '{t: $t, srss: $srss, prss: $prss, srosetta: $srosetta, prosetta: $prosetta, scpu: $scpu, pcpu: $pcpu, sctx: $sctx, pctx: $pctx,
          swake: ([.data.loops[].wakeups] | add // 0),
          pwake: ([.data.proxy.stats.loops[]?.wakeups] | add // 0),
          queries: .data.db.queries_total,
          tasks: .data.server.tokio_alive_tasks,
          routes: (.data.proxy.stats.routes // -1),
          statements: .data.db.top_statements,
          loops: [.data.loops[] | {name, wakeups}],
          ploops: [.data.proxy.stats.loops[]? | {name, wakeups}]}' > "$1"
}

# Statements that ran during the window, with their counts.
window_statements() {
    jq -n --slurpfile a "$1" --slurpfile b "$2" '
        ($a[0].statements | map({key: .sql, value: .count}) | from_entries) as $before
        | [$b[0].statements[] | {sql, count: (.count - ($before[.sql] // 0))} | select(.count > 0)]'
}

# Full table scans in the query plans of the given statements (JSON array).
full_scans() {
    python3 - "$DB" "$1" <<'PY'
import json, re, sqlite3, sys
db, statements = sys.argv[1], json.load(open(sys.argv[2]))
# Single-row configuration tables: a scan is one row.
tiny = {"route_state", "backup_config"}
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
for st in statements:
    sql = st["sql"]
    if not re.match(r"\s*(SELECT|UPDATE|DELETE|WITH)\b", sql, re.I):
        continue
    try:
        plan = con.execute("EXPLAIN QUERY PLAN " + sql, [None] * sql.count("?")).fetchall()
    except sqlite3.Error as e:
        print(f"could not explain ({e}): {sql}")
        continue
    for row in plan:
        detail = row[-1]
        m = re.match(r"SCAN (\w+)", detail)
        if m and "USING" not in detail and m.group(1) not in tiny:
            print(f"{detail}  <-  {sql}")
PY
}

# measure <phase> <rss_server_mb> <rss_proxy_mb>
measure() {
    local phase=$1 rss_s_limit=$2 rss_p_limit=$3 a b t0 t1 minutes
    a=$(mktemp); b=$(mktemp)
    settle
    sample "$a"
    t0=$(date +%s)
    info "measuring for ${WINDOW}s…"
    sleep "$WINDOW"
    t1=$(date +%s)
    sample "$b"
    minutes=$(calc "($(jq .t "$b") - $(jq .t "$a")) / 60")

    step "Budgets (${phase})"
    info "proxy routes: $(jq .routes "$b")"
    if (( $(jq .srosetta "$b") > 0 )); then
        info "running under Rosetta: its translation cache is not counted ($(calc "$(jq .srosetta "$b") / $MB") MB server, $(calc "$(jq .prosetta "$b") / $MB") MB proxy)"
    fi
    check "${phase}: RSS tond-server" "$(calc "$(jq .srss "$b") / $MB")" "<=" "$rss_s_limit" MB
    check "${phase}: RSS tond-proxy" "$(calc "$(jq .prss "$b") / $MB")" "<=" "$rss_p_limit" MB
    check "${phase}: CPU both processes" \
        "$(calc "(($(jq .scpu "$b") - $(jq .scpu "$a")) + ($(jq .pcpu "$b") - $(jq .pcpu "$a"))) / ($minutes * 60) * 100")" \
        "<=" 0.2 "% of a core"
    check "${phase}: wakeups tond-server loops" "$(calc "($(jq .swake "$b") - $(jq .swake "$a")) / $minutes")" "<=" 6 "/min"
    check "${phase}: wakeups tond-proxy loops" "$(calc "($(jq .pwake "$b") - $(jq .pwake "$a")) / $minutes")" "<=" 2 "/min"
    check "${phase}: SQL statements" "$(calc "($(jq .queries "$b") - $(jq .queries "$a")) / $minutes")" "<=" 10 "/min"
    for svc in tond-server tond-proxy; do
        check "${phase}: journal ${svc}" "$(calc "$(journal_lines "$svc" "$t0" "$t1") / $minutes * 60")" "<=" 10 "lines/hour"
    done

    local statements scans
    statements=$(mktemp)
    window_statements "$a" "$b" > "$statements"
    info "statements during the window:"
    jq -r '.[] | "    \(.count)×  \(.sql)"' "$statements" | head -20
    scans=$(full_scans "$statements")
    if [[ -n "$scans" ]]; then
        echo -e "  ${RED}✗ ${phase}: full table scans:${NC}"
        echo "$scans" | sed 's/^/      /'
        FAILURES+=("${phase}: full table scans")
    else
        echo -e "  ${GREEN}✓${NC} ${phase}: no full table scans"
    fi
    info "loop wakeups (server): $(jq -rn --slurpfile a "$a" --slurpfile b "$b" '[range($b[0].loops | length) as $i | $b[0].loops[$i] | "\(.name)=\(.wakeups - (($a[0].loops[] | select(.name == $b[0].loops[$i].name) | .wakeups) // 0))"] | join(" ")')"
    info "loop wakeups (proxy): $(jq -rn --slurpfile a "$a" --slurpfile b "$b" '[$b[0].ploops[] as $l | "\($l.name)=\($l.wakeups - (($a[0].ploops[] | select(.name == $l.name) | .wakeups) // 0))"] | join(" ")')"
    info "OS context switches (info): server $(calc "($(jq .sctx "$b") - $(jq .sctx "$a")) / $minutes")/min, proxy $(calc "($(jq .pctx "$b") - $(jq .pctx "$a")) / $minutes")/min"
    rm -f "$a" "$b" "$statements"
}

seed_dummy_apps() {
    python3 - "$DB" "$1" <<'PY'
import sqlite3, sys, uuid, datetime
db, n = sys.argv[1], int(sys.argv[2])
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
con = sqlite3.connect(db, timeout=10)
with con:
    for i in range(n):
        con.execute(
            "INSERT INTO apps (id, name, domain, source_type, source_url, container_port, status, host_port, created_at, updated_at)"
            " VALUES (?, ?, ?, 'image', 'traefik/whoami', 80, 'running', ?, ?, ?)",
            (str(uuid.uuid4()), f"budget-{i}", f"budget-{i}.budget.test", 30000 + i, now, now),
        )
PY
}

count_dummy_certs() {
    python3 - "$DB" <<'PY'
import sqlite3, sys
con = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
print(con.execute("SELECT COUNT(*) FROM certificates WHERE domain LIKE 'budget-%'").fetchone()[0])
PY
}

remove_dummy_apps() {
    python3 - "$DB" <<'PY'
import sqlite3, sys
con = sqlite3.connect(sys.argv[1], timeout=10)
with con:
    con.execute("DELETE FROM apps WHERE name LIKE 'budget-%'")
    con.execute("DELETE FROM certificates WHERE domain LIKE 'budget-%'")
PY
}

# Open and close the system log WebSocket (over the root-only socket) n times:
# every other connection closes cleanly, the rest just drop.
websocket_soak() {
    python3 - "$SOCK" "$1" <<'PY'
import base64, os, socket, sys
path, n = sys.argv[1], int(sys.argv[2])
for i in range(n):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(5)
    s.connect(path)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((
        "GET /api/system/logs/stream HTTP/1.1\r\nHost: localhost\r\n"
        "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    ).encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = s.recv(4096)
        if not chunk:
            break
        head += chunk
    if not head.startswith(b"HTTP/1.1 101"):
        sys.exit(f"upgrade {i} failed: {head[:200]!r}")
    if i % 2 == 0:
        mask = os.urandom(4)
        s.sendall(bytes([0x88, 0x80]) + mask)  # masked close frame, no payload
    s.close()
print(f"opened and closed {n} WebSockets")
PY
}

# ── Phase 1: idle, no apps ────────────────────────────────────────────────────

apps=$(api /api/apps | jq '.data | length')
if (( apps > 0 )) && [[ "$ASSERT" == 1 ]]; then
    echo "This host has ${apps} app(s); the idle budgets are defined for a fresh install (0 apps)." >&2
    echo "Run it on a fresh install (just vm-budget does that)." >&2
    exit 1
fi
step "Idle, ${apps} apps"
measure "0 apps" 15 12

step "Log WebSocket soak"
baseline=$(api /api/system/self | jq .data.server.tokio_alive_tasks)
websocket_soak 200 | sed 's/^/  /'
# Closed streams wind down asynchronously; give them a few seconds.
for _ in $(seq 1 10); do
    after=$(api /api/system/self | jq .data.server.tokio_alive_tasks)
    (( after <= baseline )) && break
    sleep 1
done
info "Tokio tasks: ${baseline} before, ${after} after"
check "WebSocket soak: leaked tasks" "$((after > baseline ? after - baseline : 0))" "<=" 0 tasks

# ── Phase 2: N dummy apps ─────────────────────────────────────────────────────

if (( APPS > 0 )); then
    step "Seeding ${APPS} dummy apps"
    remove_dummy_apps
    seed_dummy_apps "$APPS"
    # Written behind tond-server's back: its hook did not see it, so the
    # proxy picks the routes up with its fallback check (≤ 60 s).
    for _ in $(seq 1 30); do
        routes=$(api /api/system/self | jq '.data.proxy.stats.routes // 0')
        (( routes >= APPS )) && break
        sleep 3
    done
    info "tond-proxy routes ${routes}"
    (( routes >= APPS )) || FAILURES+=("the proxy did not load the ${APPS} dummy routes (has ${routes})")
    # New routes get certificates (one ACME order each). Budgets are for the
    # steady state, so wait until the proxy has settled every new domain.
    info "waiting for the certificate loop to settle the ${APPS} new domains…"
    deadline=$((SECONDS + 600))
    until (( $(count_dummy_certs) >= APPS )); do
        (( SECONDS < deadline )) || { FAILURES+=("certificates for the dummy apps did not settle in 10 min"); break; }
        sleep 5
    done
    sleep 30
    measure "${APPS} apps" 40 25
    remove_dummy_apps
fi

if [[ -n "$JSON_OUT" ]]; then
    echo "$RESULTS" | jq . > "$JSON_OUT"
fi

echo
if (( ${#FAILURES[@]} > 0 )); then
    echo -e "${RED}${BOLD}${#FAILURES[@]} budget(s) exceeded:${NC}"
    printf '  - %s\n' "${FAILURES[@]}"
    [[ "$ASSERT" == 1 ]] && exit 1
    echo -e "${YELLOW}(--no-assert: not failing)${NC}"
else
    echo -e "${GREEN}${BOLD}All budgets met.${NC}"
fi
