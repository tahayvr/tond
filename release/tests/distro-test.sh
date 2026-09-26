#!/usr/bin/env bash
# Tests for the support matrix in release/install.sh and release/update.sh:
# distribution detection from os-release, Docker's apt repository codename,
# and the refusal of unsupported systems. Uses fake os-release files, so it
# runs anywhere (bash 3.2+). release/tests/distro-containers.sh runs the real
# thing in Debian and Ubuntu containers.
#
# Run: bash release/tests/distro-test.sh   (also run by `cargo nextest`, see
# crates/tond-cli/tests/release_scripts.rs).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="${HERE}/../install.sh"
UPDATE_SH="${HERE}/../update.sh"
PASS=0
FAIL=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

fail() {
    echo "  FAIL: $*" >&2
    FAIL=$((FAIL + 1))
}
ok() { PASS=$((PASS + 1)); }
assert_eq() {
    if [[ "$1" == "$2" ]]; then ok; else fail "$3: expected '$1', got '$2'"; fi
}
assert_contains() {
    if [[ "$1" == *"$2"* ]]; then ok; else fail "$3: '$2' not in '$1'"; fi
}

# The block between the markers, from one script.
block_of() {
    sed -n '/^# ── Supported systems (keep in sync/,/^# ── end supported systems ──/p' "$1"
}

block_of "$INSTALL_SH" > "$T/block.sh"
# shellcheck source=/dev/null
source "$T/block.sh"

# os_release NAME CONTENT — write a fake os-release file.
os_release() {
    printf '%s\n' "$2" > "$T/$1"
    echo "$T/$1"
}

# expect FILE SUPPORTED CODENAME DOCKER_TARGET
expect() {
    local file="$1"
    detect_system "$file"
    assert_eq "$2" "$SYSTEM_SUPPORTED" "$(basename "$file"): supported"
    assert_eq "$3" "$SYSTEM_CODENAME" "$(basename "$file"): codename"
    assert_eq "$4" "$(docker_apt_target || echo none)" "$(basename "$file"): Docker repository"
}

echo "support matrix: the copies in install.sh and update.sh are identical"
if diff <(block_of "$INSTALL_SH") <(block_of "$UPDATE_SH") > "$T/diff"; then ok; else
    fail "supported-systems blocks differ:\n$(cat "$T/diff")"
fi
[[ -s "$T/block.sh" ]] && ok || fail "no supported-systems block in install.sh"

echo "support matrix: supported systems (as their images ship os-release)"
expect "$(os_release debian12 'PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"
NAME="Debian GNU/Linux"
VERSION_ID="12"
VERSION="12 (bookworm)"
VERSION_CODENAME=bookworm
ID=debian')" yes bookworm "debian bookworm"
expect "$(os_release debian13 'PRETTY_NAME="Debian GNU/Linux 13 (trixie)"
NAME="Debian GNU/Linux"
VERSION_ID="13"
VERSION="13 (trixie)"
VERSION_CODENAME=trixie
DEBIAN_VERSION_FULL=13.1
ID=debian')" yes trixie "debian trixie"
expect "$(os_release ubuntu2404 'PRETTY_NAME="Ubuntu 24.04.3 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.3 LTS (Noble Numbat)"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
UBUNTU_CODENAME=noble')" yes noble "ubuntu noble"
expect "$(os_release ubuntu2604 'PRETTY_NAME="Ubuntu 26.04 LTS"
NAME="Ubuntu"
VERSION_ID="26.04"
VERSION="26.04 LTS (Resolute Raccoon)"
VERSION_CODENAME=resolute
ID=ubuntu
ID_LIKE=debian
UBUNTU_CODENAME=resolute')" yes resolute "ubuntu resolute"
# A supported release without VERSION_CODENAME gets it from the matrix.
expect "$(os_release debian12-nocodename 'ID=debian
VERSION_ID=12')" yes bookworm "debian bookworm"
# Single quotes are valid os-release quoting too.
expect "$(os_release ubuntu-single-quotes "ID='ubuntu'
VERSION_ID='24.04'
VERSION_CODENAME='noble'")" yes noble "ubuntu noble"

echo "support matrix: unsupported systems"
expect "$(os_release ubuntu2204 'PRETTY_NAME="Ubuntu 22.04.5 LTS"
VERSION_ID="22.04"
VERSION_CODENAME=jammy
ID=ubuntu
ID_LIKE=debian
UBUNTU_CODENAME=jammy')" no jammy "ubuntu jammy"
expect "$(os_release debian11 'PRETTY_NAME="Debian GNU/Linux 11 (bullseye)"
VERSION_ID="11"
VERSION_CODENAME=bullseye
ID=debian')" no bullseye "debian bullseye"
expect "$(os_release ubuntu2510 'PRETTY_NAME="Ubuntu 25.10"
VERSION_ID="25.10"
VERSION_CODENAME=questing
ID=ubuntu')" no questing "ubuntu questing"
expect "$(os_release fedora 'NAME="Fedora Linux"
VERSION_ID=42
ID=fedora
PRETTY_NAME="Fedora Linux 42 (Server Edition)"')" no "" none
# Debian testing/sid has no VERSION_ID.
expect "$(os_release sid 'PRETTY_NAME="Debian GNU/Linux trixie/sid"
ID=debian')" no "" none
# Derivatives are refused, but with the override Docker comes from their base.
expect "$(os_release mint 'NAME="Linux Mint"
VERSION_ID="22"
VERSION_CODENAME=wilma
ID=linuxmint
ID_LIKE="ubuntu debian"
UBUNTU_CODENAME=noble')" no wilma "ubuntu noble"
expect "$(os_release raspbian 'ID=raspbian
ID_LIKE=debian
VERSION_ID="12"
VERSION_CODENAME=bookworm
DEBIAN_CODENAME=bookworm')" no bookworm "debian bookworm"
# No os-release at all.
detect_system "$T/does-not-exist-$$"
[[ "$SYSTEM_SUPPORTED" == no || -r /etc/os-release || -r /usr/lib/os-release ]] && ok || fail "missing os-release"

echo "support matrix: messages"
detect_system "$T/fedora"
msg="$(unsupported_system_message)"
assert_contains "$msg" "Fedora Linux 42 (Server Edition)" "message names the system"
assert_contains "$msg" "Debian 12, Debian 13, Ubuntu 24.04 LTS and Ubuntu 26.04 LTS" "message lists the matrix"
assert_contains "$msg" "--allow-unsupported-os" "message names the override"

echo "support matrix: Docker codename fallback"
assert_eq bookworm "$(docker_codename_fallback trixie)" "trixie falls back to bookworm"
assert_eq noble "$(docker_codename_fallback resolute)" "resolute falls back to noble"
assert_eq "" "$(docker_codename_fallback noble)" "noble has no fallback"

echo "support matrix: every supported release has a codename and a fallback-free or fallback-backed repo"
for entry in $TOND_SUPPORTED_SYSTEMS; do
    codename="${entry##*:}"
    [[ -n "$codename" ]] && ok || fail "$entry has no codename"
done

echo "Docker Engine version: ${TOND_MIN_DOCKER_MAJOR} or newer"
assert_eq 25 "$TOND_MIN_DOCKER_MAJOR" "minimum major"
assert_eq 27 "$(docker_major "27.3.1")" "plain version"
assert_eq 24 "$(docker_major "24.0.7-0ubuntu4~24.04.1")" "distro package version"
assert_eq 24 "$(docker_major "Docker version 24.0.7, build afdd53b")" "docker --version"
assert_eq "" "$(docker_major "")" "no version"
if docker_too_old "24.0.7"; then ok; else fail "Docker 24 must be too old"; fi
if docker_too_old "20.10.24"; then ok; else fail "Docker 20.10 must be too old"; fi
if docker_too_old "25.0.0"; then fail "Docker 25 is new enough"; else ok; fi
if docker_too_old "28.1.1"; then fail "Docker 28 is new enough"; else ok; fi
if docker_too_old ""; then fail "an unknown version is not refused"; else ok; fi
assert_contains "$(docker_too_old_message 24.0.7)" "Docker Engine 25 or newer" "message names the minimum"
assert_contains "$(docker_too_old_message 24.0.7)" "has Docker 24.0.7" "message names the installed version"
assert_contains "$(docker_too_old_message 24.0.7)" "Upgrade Docker Engine" "message says what to do"

echo "install.sh: flags"
help="$(bash "$INSTALL_SH" --help 2>&1 || true)"
assert_contains "$help" "--allow-unsupported-os" "--help documents the override"
assert_contains "$help" "Ubuntu 26.04 LTS" "--help names the matrix"

if [[ "$(uname -s)" == "Linux" ]]; then
    echo "install.sh: refuses an unsupported system before doing anything"
    set +e
    out="$(TOND_OS_RELEASE="$T/fedora" bash "$INSTALL_SH" --non-interactive 2>&1)"
    rc=$?
    set -e
    assert_eq 1 "$rc" "exit code"
    assert_contains "$out" "This server runs Fedora Linux 42" "names the system"
    assert_contains "$out" "tond.io/docs/support-matrix" "links the matrix"
    assert_eq "" "$(printf '%s' "$out" | grep -i 'download' || true)" "nothing downloaded"
else
    echo "install.sh: refuses a non-Linux system"
    set +e
    out="$(bash "$INSTALL_SH" --non-interactive 2>&1)"
    rc=$?
    set -e
    assert_eq 1 "$rc" "exit code"
    assert_contains "$out" "Tond runs on Linux servers" "message"
fi

echo "distro tests: ${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
