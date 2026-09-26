#!/usr/bin/env bash
set -euo pipefail

# Start a local ACME test CA (Pebble) for the end-to-end tests.
#
# Runs as root on the test host BEFORE Tond is installed. It:
#   1. starts pebble-challtestsrv as a DNS server that resolves every name to
#      127.0.0.1, so Pebble's HTTP-01 validation hits tond-proxy on port 80;
#   2. starts Pebble (ACME directory https://localhost:14000/dir) using that DNS;
#   3. adds Pebble's API certificate authority to the system trust store, which
#      tond-proxy's ACME client (rustls platform verifier) reads;
#   4. points tond-proxy at Pebble via a systemd drop-in (TOND_ACME_DIRECTORY);
#   5. writes Pebble's issuing root to $1 (default /opt/tond-e2e/pebble-root.pem)
#      so the smoke test can verify issued certificates (E2E_CA_FILE).
#
# Pebble's issuing root is regenerated on every start, so run the smoke test
# against the same Pebble instance that issued the certificates.

# `pebble.sh --refresh-root <file>`: wait for the running Pebble and re-fetch its
# issuing root. install.sh may restart Docker (daemon.json log rotation), which
# restarts Pebble (restart policy) with a NEW root, so run.sh calls this after
# installing and before the smoke test.
if [[ "${1:-}" == "--refresh-root" ]]; then
    ROOT_OUT="${2:-/opt/tond-e2e/pebble-root.pem}"
    for _ in $(seq 1 30); do
        curl -sf https://localhost:14000/dir > /dev/null && break
        sleep 1
    done
    curl -sf https://localhost:15000/roots/0 -o "$ROOT_OUT" \
        || { echo "Pebble is not running" >&2; exit 1; }
    exit 0
fi

ROOT_OUT="${1:-/opt/tond-e2e/pebble-root.pem}"
HERE="$(cd "$(dirname "$0")" && pwd)"
PEBBLE_IMAGE="ghcr.io/letsencrypt/pebble:latest"
CHALLTESTSRV_IMAGE="ghcr.io/letsencrypt/pebble-challtestsrv:latest"

[[ $EUID -eq 0 ]] || { echo "pebble.sh must run as root" >&2; exit 1; }
command -v docker > /dev/null || { echo "docker is required" >&2; exit 1; }

docker rm -f tond-e2e-pebble tond-e2e-challtestsrv > /dev/null 2>&1 || true

docker run -d --restart unless-stopped --name tond-e2e-challtestsrv --network host "$CHALLTESTSRV_IMAGE" \
    -defaultIPv4 127.0.0.1 -defaultIPv6 "" \
    -dnsserver 127.0.0.1:8053 -http01 "" -https01 "" -tlsalpn01 "" -doh "" \
    -management 127.0.0.1:8055 > /dev/null

# PEBBLE_VA_NOSLEEP: validate immediately. PEBBLE_WFE_NONCEREJECT=0: don't
# randomly reject nonces (Pebble's default 5% would make runs flaky).
docker run -d --restart unless-stopped --name tond-e2e-pebble --network host \
    -e PEBBLE_VA_NOSLEEP=1 -e PEBBLE_WFE_NONCEREJECT=0 \
    -v "${HERE}/pebble-config.json:/e2e/pebble-config.json:ro" \
    "$PEBBLE_IMAGE" -config /e2e/pebble-config.json -dnsserver 127.0.0.1:8053 > /dev/null

# Trust the CA that signs Pebble's own HTTPS endpoints (directory + management).
docker cp tond-e2e-pebble:/test/certs/pebble.minica.pem /usr/local/share/ca-certificates/pebble-minica.crt
update-ca-certificates > /dev/null

for _ in $(seq 1 30); do
    curl -sf https://localhost:14000/dir > /dev/null && break
    sleep 1
done
curl -sf https://localhost:14000/dir > /dev/null || {
    docker logs tond-e2e-pebble >&2
    echo "Pebble did not come up" >&2
    exit 1
}

mkdir -p "$(dirname "$ROOT_OUT")"
curl -sf https://localhost:15000/roots/0 -o "$ROOT_OUT"

# Must exist before tond-proxy first starts, or it would register an ACME
# account with Let's Encrypt production instead.
mkdir -p /etc/systemd/system/tond-proxy.service.d
cat > /etc/systemd/system/tond-proxy.service.d/e2e-acme.conf <<'EOF'
[Service]
Environment=TOND_ACME_DIRECTORY=https://localhost:14000/dir
EOF
systemctl daemon-reload

echo "Pebble ready: ACME directory https://localhost:14000/dir, issuing root at ${ROOT_OUT}"
