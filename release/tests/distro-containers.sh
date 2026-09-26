#!/usr/bin/env bash
# The support matrix, for real: in a container of each supported distribution
# run the installer's system check, its preflight summary, the dependency
# install and the Docker Engine install from Docker's apt repository (key
# fingerprint check, the codename for that release). Unsupported releases must
# be refused. Containers have no systemd, so `systemctl` is a no-op stub and
# nothing is started; the full install runs on VMs (scripts/dev-vm.sh).
#
# Usage: release/tests/distro-containers.sh [image…]
#   default images: debian:12 debian:13 ubuntu:24.04 ubuntu:26.04 (supported)
#                   and ubuntu:22.04 debian:11 (must be refused)
#   TOND_TEST_REGISTRY=mirror.gcr.io/library/  pulls the default images from a
#   Docker Hub mirror (Docker Hub rate-limits anonymous pulls).
#   TOND_TEST_PLATFORM=linux/amd64  runs the other architecture (emulated).
# Needs Docker and internet access (apt, download.docker.com).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "${1:-}" == "--inside" ]]; then
    # ── In the container ──
    expect="$2"
    mkdir -p /tmp/stubs
    printf '#!/bin/sh\nexit 0\n' > /tmp/stubs/systemctl
    chmod +x /tmp/stubs/systemctl
    export PATH="/tmp/stubs:$PATH"
    . /etc/os-release
    echo "=== ${PRETTY_NAME} ($(uname -m)), expecting: ${expect}"

    bash /tond/release/tests/distro-test.sh | tail -n 1

    set +e
    (
        TOND_INSTALL_SOURCE_ONLY=1
        # shellcheck source=../install.sh
        source /tond/release/install.sh
        check_supported_system
    )
    rc=$?
    set -e
    if [[ "$expect" == "refused" ]]; then
        if [[ $rc -eq 1 ]]; then
            echo "OK: refused as expected"
            exit 0
        fi
        echo "FAIL: ${PRETTY_NAME} was not refused" >&2
        exit 1
    fi
    [[ $rc -eq 0 ]] || { echo "FAIL: ${PRETTY_NAME} was refused" >&2; exit 1; }

    export DEBIAN_FRONTEND=noninteractive
    (
        TOND_INSTALL_SOURCE_ONLY=1
        NON_INTERACTIVE=true
        source /tond/release/install.sh
        # The minimal images lack curl; the one-line installer needs it anyway.
        apt-get update -qq > /dev/null && apt-get install -y -qq curl > /dev/null
        check_prerequisites
        install_dependencies
        install_docker
    )
    echo "--- docker.list: $(cat /etc/apt/sources.list.d/docker.list)"
    docker --version
    docker compose version
    git --version
    apt-cache policy docker-ce | sed -n '1,4p'
    echo "OK: ${PRETTY_NAME}"
    exit 0
fi

# ── On the host ──
if [[ $# -gt 0 ]]; then
    images=("$@")
else
    images=()
    for image in debian:12 debian:13 ubuntu:24.04 ubuntu:26.04 ubuntu:22.04 debian:11; do
        images+=("${TOND_TEST_REGISTRY:-}${image}")
    done
fi
failed=()
for image in "${images[@]}"; do
    case "${image##*/}" in
        debian:12|debian:13|ubuntu:24.04|ubuntu:26.04) expect=supported ;;
        *) expect=refused ;;
    esac
    platform=()
    [[ -z "${TOND_TEST_PLATFORM:-}" ]] || platform=(--platform "$TOND_TEST_PLATFORM")
    if ! docker pull -q ${platform[@]+"${platform[@]}"} "$image" > /dev/null 2>&1; then
        echo "=== ${image}: image not available, skipped"
        continue
    fi
    if ! docker run --rm ${platform[@]+"${platform[@]}"} -v "${ROOT}/release:/tond/release:ro" "$image" \
        bash /tond/release/tests/distro-containers.sh --inside "$expect"; then
        failed+=("$image")
    fi
done
if [[ ${#failed[@]} -gt 0 ]]; then
    echo "FAILED: ${failed[*]}" >&2
    exit 1
fi
echo "All images passed."
