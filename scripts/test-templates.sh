#!/usr/bin/env bash
# test-templates.sh — bring each one-click template up with local Docker,
# wait until its public service answers, then tear it down.
#
#   scripts/test-templates.sh                 # every template
#   scripts/test-templates.sh umami n8n       # just these
#   TIMEOUT=600 scripts/test-templates.sh immich
#   KEEP=1 scripts/test-templates.sh ghost    # leave it running to poke at
#   HUB_MIRROR=mirror.gcr.io scripts/test-templates.sh   # dodge Hub's rate limit
#
# Each template runs as it would under Tond: inputs are generated or filled
# with test values (placeholders expand to http://<id>.localhost), and an
# override adds Tond's container hardening (no-new-privileges, the dropped
# capabilities, a pids limit). The public port is published on 127.0.0.1 and
# probed at the template's health path until it returns 2xx (redirects are
# followed). Needs docker (with compose v2.23+) and python3 ≥ 3.11.
#
# Exit status: 0 when every template tested came up healthy.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMPLATES="${TEMPLATES:-$ROOT/templates}"
TIMEOUT="${TIMEOUT:-300}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/tond-templates.XXXXXX")"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }
docker compose version >/dev/null 2>&1 || { echo "docker compose v2 is required" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }

if [ "$#" -gt 0 ]; then
	IDS=("$@")
else
	IDS=()
	for d in "$TEMPLATES"/*/; do
		name="$(basename "$d")"
		case "$name" in _* | .*) continue ;; esac
		IDS+=("$name")
	done
fi

# Writes <dir>/.env (inputs), <dir>/meta (service port health) and
# <dir>/tond.override.yml (hardening) for one template.
render() {
	python3 - "$1" "$2" <<'PY'
import os, secrets, string, sys, tomllib

tpl_dir, out = sys.argv[1], sys.argv[2]
with open(os.path.join(tpl_dir, "template.toml"), "rb") as f:
    m = tomllib.load(f)
host = f"{m['id']}.localhost"
ctx = {"{app_url}": f"http://{host}", "{app_host}": host, "{app_name}": m["id"]}

def expand(s):
    for k, v in ctx.items():
        s = s.replace(k, v)
    return s

def generate(kind):
    if kind == "password":
        return "".join(secrets.choice(string.ascii_letters + string.digits) for _ in range(24))
    if kind == "hex":
        return secrets.token_hex(32)
    import base64
    return base64.b64encode(secrets.token_bytes(48)).decode()

samples = {"email": "admin@example.com", "url": "https://example.com", "password": "Test-password-1234", "text": "test"}
lines = []
for i in m.get("inputs", []):
    if "value" in i:
        v = expand(i["value"])
    elif "generate" in i:
        v = generate(i["generate"])
    elif "default" in i:
        v = expand(i["default"])
    else:
        v = samples[i.get("kind", "text")]
    lines.append(f"{i['key']}='{v}'")
with open(os.path.join(out, ".env"), "w") as f:
    f.write("\n".join(lines) + "\n")
p = m["public"]
with open(os.path.join(out, "meta"), "w") as f:
    f.write(f"{p['service']} {p['port']} {p.get('health_path', '/')}\n")
with open(os.path.join(out, "volumes"), "w") as f:
    for v in m.get("volumes", []):
        f.write(f"{v['path']} {v.get('owner', '')}\n")
PY
}

# Create the declared volume directories with their owners, as Tond does
# before the first deploy (chown through a container: works without root).
prepare_volumes() {
	local dir="$1" path owner
	while read -r path owner; do
		[ -n "$path" ] || continue
		mkdir -p "$dir/$path"
		if [ -n "$owner" ]; then
			docker run --rm -v "$dir/$path:/v" alpine:3 chown "$owner" /v >/dev/null 2>&1 ||
				echo "  (could not chown $path to $owner)" >&2
		fi
	done <"$dir/volumes"
}

harden() {
	local dir="$1" services svc caps
	services="$(docker compose --project-directory "$dir" -f "$dir/compose.yml" config --services)" || return 1
	{
		echo "services:"
		for svc in $services; do
			caps="$(docker compose --project-directory "$dir" -f "$dir/compose.yml" config --format json |
				python3 -c "import json,sys; d=json.load(sys.stdin)['services']['$svc']; print(' '.join(d.get('cap_add') or []))")"
			echo "  $svc:"
			echo "    security_opt: [\"no-new-privileges:true\"]"
			echo "    pids_limit: 4096"
			echo "    cap_drop:"
			for cap in NET_RAW MKNOD AUDIT_WRITE SETFCAP; do
				case " $caps " in *" $cap "*) ;; *) echo "      - $cap" ;; esac
			done
		done
	} >"$dir/tond.override.yml"
}

# With HUB_MIRROR (e.g. mirror.gcr.io), pull missing Docker Hub images through
# the mirror and tag them with their Hub name: avoids Hub's anonymous rate limit.
prefetch() {
	local dir="$1" image first path
	[ -n "${HUB_MIRROR:-}" ] || return 0
	for image in $(docker compose --project-directory "$dir" --env-file "$dir/.env" -f "$dir/compose.yml" config --images); do
		docker image inspect "$image" >/dev/null 2>&1 && continue
		path="$image"
		case "$image" in
		*/*)
			first="${image%%/*}"
			# ghcr.io/…, host:5000/…, localhost/…: not Docker Hub
			case "$first" in *.* | *:* | localhost) continue ;; esac
			;;
		*) path="library/$image" ;;
		esac
		docker pull -q "$HUB_MIRROR/$path" >/dev/null && docker tag "$HUB_MIRROR/$path" "$image"
	done
}

cleanup_dir() {
	local dir="$1"
	rm -rf "$dir" 2>/dev/null ||
		docker run --rm -v "$dir:/w" alpine:3 sh -c 'rm -rf /w/* /w/.[!.]*' >/dev/null 2>&1
	rm -rf "$dir" 2>/dev/null || true
}

PASSED=()
FAILED=()

for id in "${IDS[@]}"; do
	src="$TEMPLATES/$id"
	if [ ! -f "$src/template.toml" ]; then
		echo "✗ $id: no such template" >&2
		FAILED+=("$id")
		continue
	fi
	dir="$WORK/$id"
	mkdir -p "$dir"
	cp "$src/compose.yml" "$dir/compose.yml"
	render "$src" "$dir" || { FAILED+=("$id"); continue; }
	read -r service port health <"$dir/meta"
	project="tondtpl-$id"
	dc=(docker compose -p "$project" --project-directory "$dir" --env-file "$dir/.env" -f "$dir/compose.yml")
	if ! harden "$dir"; then
		echo "✗ $id: compose config failed" >&2
		FAILED+=("$id")
		continue
	fi
	dc+=(-f "$dir/tond.override.yml")
	prefetch "$dir"
	prepare_volumes "$dir"

	echo "▶ $id: starting ($service:$port, health $health)"
	started=$(date +%s)
	if ! "${dc[@]}" up -d --quiet-pull >"$dir/up.log" 2>&1; then
		echo "✗ $id: docker compose up failed" >&2
		tail -n 20 "$dir/up.log" >&2
		"${dc[@]}" down -v --remove-orphans >/dev/null 2>&1
		FAILED+=("$id")
		continue
	fi

	host_port="$("${dc[@]}" port "$service" "$port" 2>/dev/null | sed 's/.*://')"
	ok=0
	while [ $(($(date +%s) - started)) -lt "$TIMEOUT" ]; do
		if [ -n "$host_port" ] && curl -fsL -o /dev/null -m 5 "http://127.0.0.1:$host_port$health"; then
			ok=1
			break
		fi
		[ -z "$host_port" ] && host_port="$("${dc[@]}" port "$service" "$port" 2>/dev/null | sed 's/.*://')"
		sleep 3
	done
	elapsed=$(($(date +%s) - started))
	mem="$(docker stats --no-stream --format '{{.MemUsage}}' $("${dc[@]}" ps -q) 2>/dev/null |
		python3 -c "
import sys,re
t=0
for l in sys.stdin:
    m=re.match(r'([\d.]+)([KMG]i?B)', l.strip())
    if m:
        v=float(m.group(1)); u=m.group(2)[0]
        t+=v/1024 if u=='K' else v*1024 if u=='G' else v
print(f'{t:.0f} MiB')")"

	if [ "$ok" = 1 ]; then
		echo "✓ $id: healthy after ${elapsed}s, using $mem"
		PASSED+=("$id ($mem)")
	else
		echo "✗ $id: no 2xx from $health within ${TIMEOUT}s" >&2
		"${dc[@]}" ps >&2
		"${dc[@]}" logs --tail 30 >&2
		FAILED+=("$id")
	fi

	if [ "${KEEP:-0}" = 1 ]; then
		echo "  kept running: http://127.0.0.1:$host_port  (${dc[*]} down -v)"
	else
		"${dc[@]}" down -v --remove-orphans >/dev/null 2>&1
		cleanup_dir "$dir"
	fi
done

[ "${KEEP:-0}" = 1 ] || cleanup_dir "$WORK"

echo
echo "Healthy (${#PASSED[@]}): ${PASSED[*]:-none}"
echo "Failed (${#FAILED[@]}): ${FAILED[*]:-none}"
[ "${#FAILED[@]}" -eq 0 ]
