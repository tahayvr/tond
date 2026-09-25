#!/usr/bin/env bash
set -euo pipefail

# Local Tond test VM on macOS, using OrbStack (https://orbstack.dev).
#
# Creates an x86_64 Ubuntu 24.04 machine (run through Rosetta on Apple Silicon
# — install.sh only supports x86_64), installs Tond from the bundle built by
# `just package`, and runs the end-to-end smoke test inside it. OrbStack mounts
# the Mac filesystem at the same paths inside the machine, so nothing needs to
# be copied, and its SSH proxy makes the machine reachable as root@<name>@orb —
# which lets the existing deploy recipes target it:
#
#   just server=root@tond-vm@orb deploy
#
# Usage: scripts/dev-vm.sh <create|install|e2e|shell|destroy> [--name tond-vm]
#   create    create the machine (idempotent)
#   install   install/upgrade Tond from dist/tond-linux-x86_64 (dashboard at
#             https://<name>.orb.local once the proxy's cert is trusted, or
#             http://<name>.orb.local)
#   e2e       install + smoke test with a local Pebble ACME CA (resets the
#             machine's ACME state; the VM is meant to be disposable)
#   shell     open a root shell in the machine
#   destroy   delete the machine

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="${ROOT}/dist/tond-linux-x86_64"
NAME="tond-vm"
CMD="${1:-}"
shift || true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --name) NAME="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

command -v orb > /dev/null || {
    echo "OrbStack is required: brew install --cask orbstack (then open it once)" >&2
    exit 1
}

in_vm() { orb -m "$NAME" -u root "$@"; }

require_bundle() {
    [[ -f "${BUNDLE}/install.sh" ]] || { echo "No bundle at ${BUNDLE}. Run: just package" >&2; exit 1; }
}

case "$CMD" in
    create)
        if orb list -q 2> /dev/null | grep -qx "$NAME"; then
            echo "Machine ${NAME} already exists"
        else
            orb create --arch amd64 ubuntu:noble "$NAME"
        fi
        ;;
    install)
        require_bundle
        in_vm "${BUNDLE}/install.sh" --non-interactive --dashboard-domain "${NAME}.orb.local"
        echo "Tond installed in ${NAME}. Deploy changes with: just server=root@${NAME}@orb deploy"
        ;;
    e2e)
        require_bundle
        in_vm "${ROOT}/e2e/run.sh" --bundle "$BUNDLE" --acme pebble --base-domain tond.test
        ;;
    shell)
        in_vm bash -l
        ;;
    destroy)
        orb delete -f "$NAME"
        ;;
    *)
        sed -n '4,24p' "$0"
        exit 1
        ;;
esac
