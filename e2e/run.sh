#!/usr/bin/env bash
set -euo pipefail

# Install a Tond bundle on this host and run the end-to-end smoke test.
#
# Runs as root on a fresh (or disposable) Ubuntu x86_64 or aarch64 host: a GitHub Actions
# runner, the local dev VM, or a throwaway Hetzner server. It installs exactly
# what users install (release/install.sh in bundled mode), then runs smoke.sh.
#
# Usage:
#   sudo e2e/run.sh --bundle <dir> --base-domain <domain> [--preview-domain <domain>]
#                   [--apps-domain <domain>] [--acme <mode>] [--ip <ip>]
#
#   --bundle        bundle directory produced by release/package.sh
#   --base-domain   apps are served at <name>.<domain>; the dashboard at dash.<domain>
#   --preview-domain  passed to install.sh; previews are served at <id>-<app>.<domain>
#                   (a different site than the base domain, e.g. preview.test).
#                   Without it the preview checks are skipped.
#   --apps-domain   passed to install.sh; apps without a custom domain are served at
#                   <app>.<domain> (a different site than the base and preview
#                   domains, e.g. apps.test). Without it the check is skipped.
#   --acme          pebble (default): local Pebble CA, works offline / in CI
#                   letsencrypt-staging: real ACME against Let's Encrypt staging
#                     (needs public DNS for the domains, e.g. <ip>.sslip.io)
#                   none: skip certificate issuance checks
#   --ip            IP the domains resolve to (default: 127.0.0.1)
#   --test          smoke (default): e2e/smoke.sh; budget: e2e/budget.sh (idle
#                   resource budgets; run it on a fresh install)

HERE="$(cd "$(dirname "$0")" && pwd)"
BUNDLE=""
BASE_DOMAIN=""
PREVIEW_DOMAIN=""
APPS_DOMAIN=""
ACME="pebble"
IP="127.0.0.1"
TEST="smoke"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundle) BUNDLE="$2"; shift 2 ;;
        --base-domain) BASE_DOMAIN="$2"; shift 2 ;;
        --preview-domain) PREVIEW_DOMAIN="$2"; shift 2 ;;
        --apps-domain) APPS_DOMAIN="$2"; shift 2 ;;
        --acme) ACME="$2"; shift 2 ;;
        --ip) IP="$2"; shift 2 ;;
        --test) TEST="$2"; shift 2 ;;
        -h|--help) sed -n '4,28p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

[[ $EUID -eq 0 ]] || { echo "run.sh must run as root" >&2; exit 1; }
[[ -f "${BUNDLE}/install.sh" ]] || { echo "--bundle must point at a release/package.sh bundle" >&2; exit 1; }
[[ -n "$BASE_DOMAIN" ]] || { echo "--base-domain is required" >&2; exit 1; }

CA_FILE=""
mkdir -p /opt/tond-e2e

# Docker is needed by Pebble before install.sh would install it.
if ! command -v docker > /dev/null; then
    echo "Installing Docker..."
    curl -fsSL https://get.docker.com | sh > /dev/null
    systemctl enable --now docker > /dev/null
fi

# ACME configuration must be in place before tond-proxy's first start.
case "$ACME" in
    pebble)
        # Pebble forgets every account and certificate when it restarts, so on a
        # reused host (the local VM) drop the proxy's ACME state from earlier runs.
        if [[ -d /opt/tond/data ]]; then
            systemctl stop tond-proxy 2> /dev/null || true
            rm -rf /opt/tond/data/acme_account.json /opt/tond/data/certs
        fi
        "${HERE}/pebble/pebble.sh" /opt/tond-e2e/pebble-root.pem
        CA_FILE=/opt/tond-e2e/pebble-root.pem
        ;;
    letsencrypt-staging)
        mkdir -p /etc/systemd/system/tond-proxy.service.d
        printf '[Service]\nEnvironment=TOND_ACME_STAGING=1\n' \
            > /etc/systemd/system/tond-proxy.service.d/e2e-acme.conf
        systemctl daemon-reload
        # Staging issues from either the RSA (X1) or ECDSA (X2) staging root.
        CA_FILE=/opt/tond-e2e/letsencrypt-staging-roots.pem
        {
            curl -fsSL https://letsencrypt.org/certs/staging/letsencrypt-stg-root-x1.pem
            curl -fsSL https://letsencrypt.org/certs/staging/letsencrypt-stg-root-x2.pem
        } > "$CA_FILE"
        ;;
    none) ;;
    *) echo "Unknown --acme mode: $ACME" >&2; exit 1 ;;
esac

INSTALL_ARGS=(--non-interactive --dashboard-domain "dash.${BASE_DOMAIN}")
[[ -n "$PREVIEW_DOMAIN" ]] && INSTALL_ARGS+=(--preview-domain "$PREVIEW_DOMAIN")
# Always pass --apps-domain so the install never guesses one from the host's IP.
INSTALL_ARGS+=(--apps-domain "$APPS_DOMAIN")
"${BUNDLE}/install.sh" "${INSTALL_ARGS[@]}"

# install.sh may have restarted Docker (log rotation in daemon.json), which
# restarts Pebble with a new issuing root: fetch the current one.
if [[ "$ACME" == pebble ]]; then
    "${HERE}/pebble/pebble.sh" --refresh-root "$CA_FILE"
fi

case "$TEST" in
    smoke)
        E2E_BASE_DOMAIN="$BASE_DOMAIN" \
        E2E_PREVIEW_DOMAIN="$PREVIEW_DOMAIN" \
        E2E_APPS_DOMAIN="$APPS_DOMAIN" \
        E2E_TARGET_IP="$IP" \
        E2E_CA_FILE="$CA_FILE" \
            "${HERE}/smoke.sh"
        ;;
    budget) "${HERE}/budget.sh" ${BUDGET_ARGS:-} ;;
    *) echo "Unknown --test: $TEST (expected smoke or budget)" >&2; exit 1 ;;
esac
