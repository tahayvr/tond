#!/usr/bin/env bash
set -euo pipefail

# Run the end-to-end tests on a throwaway Hetzner Cloud server.
#
# Creates a fresh Ubuntu 24.04 server (x86 cx23, or ARM cax11 with --arch
# aarch64), copies the bundle + e2e scripts, runs e2e/run.sh on it, collects
# the results, and deletes the server (its one-off SSH key, and any DNS
# records it created) afterwards — even when the test fails or the script is
# interrupted.
#
# With --acme letsencrypt-staging (default) or letsencrypt, apps get real
# certificates for <name>.<ip>.sslip.io (previews on <ip>.nip.io, a different
# site) — exercising public DNS, the cloud network, and HTTP-01 validation from
# the internet, which the local Pebble runs can't. With --dns-zone, the apps
# domain is <server-name>.<zone> on Cloudflare: the script points
# *.<server-name>.<zone> at the server and the smoke test orders a wildcard
# certificate through Tond's Cloudflare DNS provider (real DNS-01).
#
# Requires: hcloud CLI with an active context (or HCLOUD_TOKEN), ssh, jq, a
# bundle from release/package.sh; for --dns-zone, CLOUDFLARE_API_TOKEN (Zone →
# DNS → Edit on that zone).
#
# Usage:
#   e2e/hcloud.sh [--arch x86_64|aarch64] [--bundle <dir>]
#                 [--acme letsencrypt-staging|letsencrypt|pebble|none]
#                 [--test smoke|budget|both] [--dns-zone <zone>]
#                 [--artifacts <dir>] [--run-id <id>]
#                 [--type cx23|cax11] [--location nbg1] [--keep]
#   e2e/hcloud.sh --gc [--max-age-hours 3] [--dns-zone <zone>]
#   e2e/hcloud.sh --delete-run <id> [--dns-zone <zone>]
#
#   --test        smoke (default), budget (resource budgets on the fresh
#                 install), both (budget first, then smoke, one server)
#   --artifacts   copy the results here: run.log, budget.json, journals,
#                 update.log, summary.txt (also on failure)
#   --run-id      label the server and key tond-e2e-run=<id> (the workflow
#                 deletes everything with its run's label at the end)
#   --keep        leave the server running for debugging (prints how to SSH in
#                 and how to delete it)
#   --gc          delete leftover e2e servers/keys (and DNS records under
#                 --dns-zone) older than --max-age-hours: a safety net for
#                 runs that were killed before cleaning up
#   --delete-run  delete the servers, keys and DNS records of one run

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="x86_64"
BUNDLE=""
ACME="letsencrypt-staging"
TEST="smoke"
DNS_ZONE="${E2E_DNS_ZONE:-}"
ARTIFACTS=""
RUN_ID=""
SERVER_TYPE="${HCLOUD_SERVER_TYPE:-}"
LOCATION="${HCLOUD_LOCATION:-nbg1}"
KEEP=false
GC=false
DELETE_RUN=""
MAX_AGE_HOURS=3
LABEL="tond-e2e=true"
CF_API="https://api.cloudflare.com/client/v4"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --bundle) BUNDLE="$2"; shift 2 ;;
        --acme) ACME="$2"; shift 2 ;;
        --test) TEST="$2"; shift 2 ;;
        --dns-zone) DNS_ZONE="$2"; shift 2 ;;
        --artifacts) ARTIFACTS="$2"; shift 2 ;;
        --run-id) RUN_ID="$2"; shift 2 ;;
        --type) SERVER_TYPE="$2"; shift 2 ;;
        --location) LOCATION="$2"; shift 2 ;;
        --keep) KEEP=true; shift ;;
        --gc) GC=true; shift ;;
        --delete-run) DELETE_RUN="$2"; shift 2 ;;
        --max-age-hours) MAX_AGE_HOURS="$2"; shift 2 ;;
        -h|--help) sed -n '4,47p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64"; SERVER_TYPE="${SERVER_TYPE:-cx23}" ;;
    aarch64|arm64) ARCH="aarch64"; SERVER_TYPE="${SERVER_TYPE:-cax11}" ;;
    *) echo "Unsupported --arch: $ARCH" >&2; exit 1 ;;
esac
case "$ACME" in letsencrypt-staging|letsencrypt|pebble|none) ;; *) echo "Unknown --acme: $ACME" >&2; exit 1 ;; esac
case "$TEST" in smoke|budget|both) ;; *) echo "Unknown --test: $TEST" >&2; exit 1 ;; esac
[[ -z "$RUN_ID" || "$RUN_ID" =~ ^[A-Za-z0-9_.-]{1,63}$ ]] || { echo "--run-id: letters, digits, _ . - only" >&2; exit 1; }
BUNDLE="${BUNDLE:-${ROOT}/dist/tond-linux-${ARCH}}"

command -v hcloud > /dev/null || { echo "hcloud CLI is required" >&2; exit 1; }
command -v jq > /dev/null || { echo "jq is required" >&2; exit 1; }
if [[ -n "$DNS_ZONE" && "$GC" == false && -z "$DELETE_RUN" && "$ACME" != letsencrypt* ]]; then
    echo "Note: --dns-zone needs a public CA (--acme letsencrypt-staging or letsencrypt); ignoring it with --acme ${ACME}"
    DNS_ZONE=""
fi
if [[ -n "$DNS_ZONE" && -z "${CLOUDFLARE_API_TOKEN:-}" ]]; then
    echo "--dns-zone needs CLOUDFLARE_API_TOKEN (Zone → DNS → Edit on ${DNS_ZONE})" >&2
    exit 1
fi

# ── Cloudflare DNS (only with --dns-zone) ─────────────────────────────────────

cf() { curl -fsS -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" -H "Content-Type: application/json" "$@"; }
cf_zone_id() {
    cf "${CF_API}/zones?name=${DNS_ZONE}" | jq -r '.result[0].id // empty'
}
# cf_delete_records <jq filter on a record>: delete the matching records.
cf_delete_records() {
    local zone_id
    zone_id="$(cf_zone_id)" || return 0
    [[ -n "$zone_id" ]] || { echo "Cloudflare: no zone ${DNS_ZONE} visible to the token" >&2; return 0; }
    cf "${CF_API}/zones/${zone_id}/dns_records?per_page=1000" |
        jq -r ".result[] | select($1) | \"\(.id) \(.type) \(.name)\"" |
        while read -r id type name; do
            echo "Deleting DNS record ${type} ${name}"
            cf -X DELETE "${CF_API}/zones/${zone_id}/dns_records/${id}" > /dev/null || true
        done
}
# Records of one server: <name>.<zone> and everything under it (the A
# records below, and _acme-challenge TXT records a killed order left behind).
cf_delete_server_records() {
    cf_delete_records "(.name == \"$1.${DNS_ZONE}\") or (.name | endswith(\".$1.${DNS_ZONE}\"))"
}

# ── Garbage collection / one run ──────────────────────────────────────────────

if [[ "$GC" == true ]]; then
    cutoff=$(( $(date +%s) - MAX_AGE_HOURS * 3600 ))
    for kind in server ssh-key; do
        hcloud "$kind" list -l "$LABEL" -o json | jq -r --argjson cutoff "$cutoff" \
            '.[] | select((.labels["created-at"] // "0" | tonumber) < $cutoff) | .name' |
            while read -r name; do
                echo "Deleting stale ${kind} ${name}"
                hcloud "$kind" delete "$name" > /dev/null
            done
    done
    if [[ -n "$DNS_ZONE" ]]; then
        cf_delete_records "(.name | test(\"(^|[.])tond-e2e-[0-9]{8}-\")) and ((.created_on | sub(\"[.][0-9]+Z$\"; \"Z\") | fromdateiso8601) < ${cutoff})"
    fi
    exit 0
fi

if [[ -n "$DELETE_RUN" ]]; then
    for kind in server ssh-key; do
        hcloud "$kind" list -l "tond-e2e-run=${DELETE_RUN}" -o json | jq -r '.[].name' |
            while read -r name; do
                echo "Deleting ${kind} ${name} (run ${DELETE_RUN})"
                hcloud "$kind" delete "$name" > /dev/null || true
                if [[ "$kind" == server && -n "$DNS_ZONE" ]]; then cf_delete_server_records "$name"; fi
            done
    done
    exit 0
fi

[[ -f "${BUNDLE}/install.sh" ]] || {
    echo "No bundle at ${BUNDLE}. Build one with: just package" >&2
    exit 1
}

# ── Provision ─────────────────────────────────────────────────────────────────

NAME="tond-e2e-$(date +%Y%m%d-%H%M%S)-${RANDOM}"
WORK="$(mktemp -d)"
KEY="${WORK}/id_ed25519"
SSH_OPTS=(-i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=10)
IP=""
CLEANED=false

collect_artifacts() {
    [[ -n "$ARTIFACTS" && -n "$IP" ]] || return 0
    mkdir -p "$ARTIFACTS"
    scp -r "${SSH_OPTS[@]}" "root@${IP}:/root/tond-e2e/results/." "$ARTIFACTS/" > /dev/null 2>&1 \
        || echo "Could not copy the results from ${NAME}" >&2
}

cleanup() {
    local code=$?
    [[ "$CLEANED" == false ]] || exit "$code"
    CLEANED=true
    trap - INT TERM
    collect_artifacts
    if [[ "$KEEP" == true ]]; then
        echo ""
        echo "Kept ${NAME} (${IP:-no ip}). SSH: ssh ${SSH_OPTS[*]} root@${IP:-?}"
        echo "Delete it with: e2e/hcloud.sh --delete-run ${RUN_ID:-<id>}${DNS_ZONE:+ --dns-zone ${DNS_ZONE}}  (or hcloud server delete ${NAME} && hcloud ssh-key delete ${NAME})"
        echo "(The private key is in ${WORK}.)"
    else
        echo "Deleting ${NAME}..."
        # Retried: a transient API error must not leave a server running.
        for _ in 1 2 3; do
            hcloud server delete "$NAME" > /dev/null 2>&1 && break
            hcloud server describe "$NAME" > /dev/null 2>&1 || break
            sleep 5
        done
        hcloud server describe "$NAME" > /dev/null 2>&1 \
            && echo "WARNING: could not delete server ${NAME}; delete it by hand (hcloud server delete ${NAME})" >&2
        hcloud ssh-key delete "$NAME" > /dev/null 2>&1 || true
        [[ -z "$DNS_ZONE" ]] || cf_delete_server_records "$NAME" || true
        rm -rf "$WORK"
    fi
    exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ssh-keygen -q -t ed25519 -N "" -C "$NAME" -f "$KEY"
LABELS=(--label "$LABEL" --label "created-at=$(date +%s)")
[[ -n "$RUN_ID" ]] && LABELS+=(--label "tond-e2e-run=${RUN_ID}")
hcloud ssh-key create --name "$NAME" --public-key-from-file "${KEY}.pub" "${LABELS[@]}" > /dev/null

echo "Creating ${SERVER_TYPE} server ${NAME} in ${LOCATION}..."
hcloud server create --name "$NAME" --type "$SERVER_TYPE" --image ubuntu-24.04 \
    --location "$LOCATION" --ssh-key "$NAME" "${LABELS[@]}" > /dev/null
IP="$(hcloud server ip "$NAME")"
echo "Server IP: ${IP}"

APPS_DOMAIN=""
if [[ -n "$DNS_ZONE" ]]; then
    APPS_DOMAIN="${NAME}.${DNS_ZONE}"
    zone_id="$(cf_zone_id)"
    [[ -n "$zone_id" ]] || { echo "Cloudflare: the token can't see the zone ${DNS_ZONE}" >&2; exit 1; }
    for record in "$APPS_DOMAIN" "*.${APPS_DOMAIN}"; do
        cf -X POST "${CF_API}/zones/${zone_id}/dns_records" \
            -d "$(jq -nc --arg n "$record" --arg ip "$IP" --arg c "tond-e2e ${NAME}" \
                '{type: "A", name: $n, content: $ip, ttl: 60, proxied: false, comment: $c}')" > /dev/null
    done
    echo "DNS: *.${APPS_DOMAIN} → ${IP} (Cloudflare)"
fi

echo "Waiting for SSH..."
for _ in $(seq 1 60); do
    ssh "${SSH_OPTS[@]}" "root@${IP}" true 2> /dev/null && break
    sleep 5
done
ssh "${SSH_OPTS[@]}" "root@${IP}" "cloud-init status --wait > /dev/null 2>&1 || true"

# ── Run ───────────────────────────────────────────────────────────────────────

echo "Uploading bundle and e2e scripts..."
mkdir -p "${WORK}/payload"
cp -R "$BUNDLE" "${WORK}/payload/bundle"
cp -R "${ROOT}/e2e" "${WORK}/payload/e2e"
# macOS (bsdtar): don't add ._* files or extended-attribute headers GNU tar warns about.
TAR_FLAGS=()
tar --version 2> /dev/null | grep -q bsdtar && TAR_FLAGS=(--no-mac-metadata --no-xattrs)
COPYFILE_DISABLE=1 tar "${TAR_FLAGS[@]}" -czf "${WORK}/payload.tar.gz" -C "${WORK}/payload" bundle e2e
scp "${SSH_OPTS[@]}" "${WORK}/payload.tar.gz" "root@${IP}:/root/payload.tar.gz"
ssh "${SSH_OPTS[@]}" "root@${IP}" "mkdir -p /root/tond-e2e/results && tar -xzf /root/payload.tar.gz -C /root/tond-e2e"
# Secrets go through stdin into a root-only file, never onto a command line.
{
    if [[ -n "$DNS_ZONE" ]]; then
        printf 'E2E_CLOUDFLARE_TOKEN=%q\n' "$CLOUDFLARE_API_TOKEN"
        printf 'E2E_CLOUDFLARE_ZONE=%q\n' "$DNS_ZONE"
    fi
} | ssh "${SSH_OPTS[@]}" "root@${IP}" "umask 077 && cat > /root/tond-e2e/secrets.env"

# The preview domain must be a different site from the dashboard's.
case "$ACME" in
    letsencrypt-staging|letsencrypt)
        BASE_DOMAIN="${IP//./-}.sslip.io"; PREVIEW_DOMAIN="${IP//./-}.nip.io"; TARGET_IP="$IP" ;;
    *) BASE_DOMAIN="tond.test"; PREVIEW_DOMAIN="preview.test"; TARGET_IP="127.0.0.1" ;;
esac

echo "Running e2e (test=${TEST}, acme=${ACME}, domains=*.${BASE_DOMAIN}, previews=*.${PREVIEW_DOMAIN}${APPS_DOMAIN:+, apps=*.${APPS_DOMAIN}})..."
RUN_ARGS=(--bundle /root/tond-e2e/bundle --acme "$ACME" --base-domain "$BASE_DOMAIN"
    --preview-domain "$PREVIEW_DOMAIN" --ip "$TARGET_IP" --test "$TEST"
    --results /root/tond-e2e/results --env-file /root/tond-e2e/secrets.env)
[[ -n "$APPS_DOMAIN" ]] && RUN_ARGS+=(--apps-domain "$APPS_DOMAIN")
LOG="${WORK}/run.log"
[[ -n "$ARTIFACTS" ]] && mkdir -p "$ARTIFACTS" && LOG="${ARTIFACTS}/run.log"
ssh "${SSH_OPTS[@]}" "root@${IP}" "/root/tond-e2e/e2e/run.sh $(printf '%q ' "${RUN_ARGS[@]}")" 2>&1 | tee "$LOG"
