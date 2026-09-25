#!/usr/bin/env bash
set -euo pipefail

# Run the end-to-end smoke test on a throwaway Hetzner Cloud server.
#
# Creates a fresh Ubuntu 24.04 x86 server, copies the bundle + e2e scripts,
# runs e2e/run.sh on it, and deletes the server (and its one-off SSH key)
# afterwards — even when the test fails or the script is interrupted.
#
# With the default --acme letsencrypt-staging, apps get real certificates from
# Let's Encrypt staging for <name>.<ip>.sslip.io — exercising public DNS, the
# cloud network, and HTTP-01 validation from the internet, which the local
# Pebble runs can't.
#
# Requires: hcloud CLI with an active context (or HCLOUD_TOKEN), ssh, a bundle
# from release/package.sh.
#
# Usage:
#   e2e/hcloud.sh [--bundle <dir>] [--acme letsencrypt-staging|pebble|none]
#                 [--type cx23] [--location nbg1] [--keep]
#   e2e/hcloud.sh --gc [--max-age-hours 3]
#
#   --keep   leave the server running for debugging (prints how to SSH in and
#            how to delete it)
#   --gc     delete leftover e2e servers/keys older than --max-age-hours
#            (a safety net for runs that were killed before cleaning up)

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="${ROOT}/dist/tond-linux-x86_64"
ACME="letsencrypt-staging"
SERVER_TYPE="${HCLOUD_SERVER_TYPE:-cx23}"
LOCATION="${HCLOUD_LOCATION:-nbg1}"
KEEP=false
GC=false
MAX_AGE_HOURS=3
LABEL="tond-e2e=true"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundle) BUNDLE="$2"; shift 2 ;;
        --acme) ACME="$2"; shift 2 ;;
        --type) SERVER_TYPE="$2"; shift 2 ;;
        --location) LOCATION="$2"; shift 2 ;;
        --keep) KEEP=true; shift ;;
        --gc) GC=true; shift ;;
        --max-age-hours) MAX_AGE_HOURS="$2"; shift 2 ;;
        -h|--help) sed -n '4,27p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

command -v hcloud > /dev/null || { echo "hcloud CLI is required" >&2; exit 1; }

# ── Garbage collection ────────────────────────────────────────────────────────

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

cleanup() {
    local code=$?
    if [[ "$KEEP" == true ]]; then
        echo ""
        echo "Kept ${NAME} (${IP:-no ip}). SSH: ssh ${SSH_OPTS[*]} root@${IP:-?}"
        echo "Delete it with: hcloud server delete ${NAME} && hcloud ssh-key delete ${NAME}"
        echo "(The private key is in ${WORK}.)"
    else
        echo "Deleting ${NAME}..."
        hcloud server delete "$NAME" > /dev/null 2>&1 || true
        hcloud ssh-key delete "$NAME" > /dev/null 2>&1 || true
        rm -rf "$WORK"
    fi
    exit "$code"
}
trap cleanup EXIT INT TERM

ssh-keygen -q -t ed25519 -N "" -C "$NAME" -f "$KEY"
LABELS=(--label "$LABEL" --label "created-at=$(date +%s)")
hcloud ssh-key create --name "$NAME" --public-key-from-file "${KEY}.pub" "${LABELS[@]}" > /dev/null

echo "Creating ${SERVER_TYPE} server ${NAME} in ${LOCATION}..."
hcloud server create --name "$NAME" --type "$SERVER_TYPE" --image ubuntu-24.04 \
    --location "$LOCATION" --ssh-key "$NAME" "${LABELS[@]}" > /dev/null
IP="$(hcloud server ip "$NAME")"
echo "Server IP: ${IP}"

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
ssh "${SSH_OPTS[@]}" "root@${IP}" "mkdir -p /root/tond-e2e && tar -xzf /root/payload.tar.gz -C /root/tond-e2e"

case "$ACME" in
    letsencrypt-staging) BASE_DOMAIN="${IP//./-}.sslip.io"; TARGET_IP="$IP" ;;
    *) BASE_DOMAIN="tond.test"; TARGET_IP="127.0.0.1" ;;
esac

echo "Running e2e (acme=${ACME}, domains=*.${BASE_DOMAIN})..."
ssh "${SSH_OPTS[@]}" "root@${IP}" \
    "/root/tond-e2e/e2e/run.sh --bundle /root/tond-e2e/bundle --acme ${ACME} --base-domain ${BASE_DOMAIN} --ip ${TARGET_IP}"
