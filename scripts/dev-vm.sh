#!/usr/bin/env bash
set -euo pipefail

# Local Tond test VM on macOS, using OrbStack (https://orbstack.dev).
#
# Creates an Ubuntu 24.04 machine — x86_64 (run through Rosetta on Apple
# Silicon) or arm64 (native on Apple Silicon, and much faster) — installs Tond
# from the bundle built by `just package` for that architecture, and runs the
# end-to-end smoke test inside it. OrbStack mounts the Mac filesystem at the
# same paths inside the machine, so nothing needs to be copied, and its SSH
# proxy makes the machine reachable as root@<name>@orb — which lets the
# existing deploy recipes target it:
#
#   just server=root@tond-vm@orb deploy
#
# Usage: scripts/dev-vm.sh <create|install|e2e|budget|shell|destroy> [--arch amd64|arm64] [--name <machine>]
#   --arch    amd64 (alias x86_64, default) or arm64 (alias aarch64)
#   --name    machine name (default: tond-vm for amd64, tond-vm-arm64 for arm64)
#
#   create    create the machine (idempotent)
#   install   install/upgrade Tond from dist/tond-linux-<arch> (dashboard at
#             https://<name>.orb.local once the proxy's cert is trusted)
#   e2e       wipe Tond's data and containers, then install + smoke test with a
#             local Pebble ACME CA and the preview domain preview.test (the VM
#             is meant to be disposable)
#   budget    like e2e, but runs e2e/budget.sh (idle resource budgets) on the
#             fresh install instead of the smoke test (BUDGET_ARGS passes
#             options, e.g. BUDGET_ARGS="--window 60")
#   shell     open a root shell in the machine
#   destroy   delete the machine

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="amd64"
NAME=""
CMD="${1:-}"
shift || true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

case "$ARCH" in
    amd64|x86_64) ORB_ARCH="amd64"; BUNDLE_ARCH="x86_64"; DEFAULT_NAME="tond-vm" ;;
    arm64|aarch64) ORB_ARCH="arm64"; BUNDLE_ARCH="aarch64"; DEFAULT_NAME="tond-vm-arm64" ;;
    *) echo "Unsupported --arch: ${ARCH} (expected amd64 or arm64)" >&2; exit 1 ;;
esac
NAME="${NAME:-$DEFAULT_NAME}"
BUNDLE="${ROOT}/dist/tond-linux-${BUNDLE_ARCH}"

command -v orb > /dev/null || {
    echo "OrbStack is required: brew install --cask orbstack (then open it once)" >&2
    exit 1
}

in_vm() { orb -m "$NAME" -u root "$@"; }

# An amd64 machine on Apple Silicon runs under Rosetta, which needs
# /proc/sys/vm/mmap_min_addr; the units' ProcSubset=pid hides it and the
# services die on start. Real x86_64 hosts don't need this, so it lives only
# in the VM (in /etc, so it survives reinstalls).
rosetta_workaround() {
    [[ "$ORB_ARCH" == amd64 && "$(uname -m)" == arm64 ]] || return 0
    in_vm bash -c 'for unit in tond-server tond-proxy; do
        mkdir -p /etc/systemd/system/${unit}.service.d
        printf "[Service]\nProcSubset=all\n" > /etc/systemd/system/${unit}.service.d/zz-rosetta.conf
    done; systemctl daemon-reload'
}

require_bundle() {
    [[ -f "${BUNDLE}/install.sh" ]] || {
        echo "No bundle at ${BUNDLE}. Run: just arch=${BUNDLE_ARCH} package" >&2
        exit 1
    }
}

case "$CMD" in
    create)
        if orb list -q 2> /dev/null | grep -qx "$NAME"; then
            echo "Machine ${NAME} already exists"
        else
            orb create --arch "$ORB_ARCH" ubuntu:noble "$NAME"
        fi
        ;;
    install)
        require_bundle
        rosetta_workaround
        in_vm "${BUNDLE}/install.sh" --non-interactive --dashboard-domain "${NAME}.orb.local"
        echo "Tond installed in ${NAME}. Deploy changes with: just server=root@${NAME}@orb deploy"
        ;;
    e2e|budget)
        require_bundle
        # Start from a clean slate: a database from an older build may carry
        # migrations this build no longer has, and leftover apps would collide.
        in_vm bash -c 'systemctl stop tond-server tond-proxy 2> /dev/null || true
            docker ps -aq --filter label=managed-by=tond | xargs -r docker rm -f > /dev/null
            rm -rf /opt/tond/data'
        rosetta_workaround
        test="smoke"
        [[ "$CMD" == budget ]] && test="budget"
        in_vm env BUDGET_ARGS="${BUDGET_ARGS:-}" "${ROOT}/e2e/run.sh" --bundle "$BUNDLE" --acme pebble \
            --base-domain tond.test --preview-domain preview.test --test "$test"
        ;;
    shell)
        in_vm bash -l
        ;;
    destroy)
        orb delete -f "$NAME"
        ;;
    *)
        sed -n '4,31p' "$0"
        exit 1
        ;;
esac
