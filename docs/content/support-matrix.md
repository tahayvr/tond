# Supported systems

Tond supports four Linux distributions on two CPU architectures. The installer and the update script check the system before they change anything, and refuse the rest with a clear message.

## Distributions

| Distribution | Version | Codename | Docker packages from |
| --- | --- | --- | --- |
| Debian | 12 | bookworm | `download.docker.com/linux/debian bookworm` |
| Debian | 13 | trixie | `download.docker.com/linux/debian trixie` |
| Ubuntu | 24.04 LTS | noble | `download.docker.com/linux/ubuntu noble` |
| Ubuntu | 26.04 LTS | resolute | `download.docker.com/linux/ubuntu resolute` |

Each is supported on **x86_64** (amd64) and **aarch64** (ARM64). Releases ship one bundle per architecture (`tond-linux-x86_64`, `tond-linux-aarch64`), and the installer picks the one for the server.

The system is read from `/etc/os-release` (`ID` and `VERSION_ID`), so minimal images and cloud images of these releases work the same as full installs. Derivatives (Linux Mint, Pop!_OS, Raspberry Pi OS…) and other releases of Debian and Ubuntu (Ubuntu 22.04, interim releases such as 25.10, Debian testing) are not supported.

## Docker

Tond installs Docker Engine from Docker's own apt repository, not from the distribution's packages, and trusts that repository only after checking its signing key's fingerprint (`9DC8 5822 9FC7 DD38 854A E2D8 8D81 803C 0EBF CD88`). The repository line uses the codename of your release.

When a new release comes out before Docker publishes packages for it, the installer falls back to the previous release's packages (Debian 13 → bookworm, Ubuntu 26.04 → noble) and says so. If Docker is already installed, the installer leaves it alone, as long as it is **Docker Engine 25 or newer**: one-click templates use compose healthcheck options that older engines don't have. With an older engine, the installer and `update.sh` stop before changing anything and ask you to upgrade Docker first. A rollback only warns.

## Hardware

| | Minimum | Recommended |
| --- | --- | --- |
| Memory | 2 GB | 4 GB or more |
| Disk | 20 GB free on `/` | more for images, volumes and local snapshots |
| CPU | 1 vCPU | 2 vCPUs (builds are the heaviest thing Tond does) |
| Network | a public IPv4 address, ports 80 and 443 reachable | |

The installer's preflight summary warns when memory or disk is below these numbers, or when ports 80 and 443 are already taken. It doesn't refuse: you know your workload. What Tond itself uses is in [Resource footprint](/operations/footprint).

## Unsupported systems

On anything else the installer stops before asking questions or downloading anything:

```text
[tond] Tond supports Debian 12, Debian 13, Ubuntu 24.04 LTS and Ubuntu 26.04 LTS (x86_64 or aarch64). This server runs Ubuntu 22.04.5 LTS.
[tond] Install on a supported system, or rerun with --allow-unsupported-os to try anyway (not supported: things may break).
[tond] Supported systems: https://tond.io/docs/support-matrix
```

To try anyway, pass `--allow-unsupported-os` (or set `TOND_ALLOW_UNSUPPORTED_OS=1`):

```sh
curl -fsSL https://tond.io/install | sudo bash -s -- --allow-unsupported-os
```

The installer warns and continues. On a Debian or Ubuntu derivative it takes Docker from the release the derivative is based on (`UBUNTU_CODENAME` or `DEBIAN_CODENAME` in `/etc/os-release`); elsewhere, install Docker Engine yourself first. The choice is recorded in `/opt/tond/ALLOW_UNSUPPORTED_OS`, so later updates warn instead of refusing.

> [!WARNING]
> Unsupported means untested. Tond may install and run, but problems specific to that system won't be fixed, and an update may stop working on it.

## How the matrix is tested

- `release/tests/distro-test.sh` checks the detection logic against the `os-release` files of every supported release and of common unsupported ones, and checks that `install.sh` and `update.sh` carry the same list. It runs with the Rust tests.
- `release/tests/distro-containers.sh` runs the installer's system check, preflight, dependency install and Docker install in real `debian:12`, `debian:13`, `ubuntu:24.04` and `ubuntu:26.04` containers (both architectures), and checks that `ubuntu:22.04` and `debian:11` are refused.
- The end-to-end test (`e2e/smoke.sh`) installs Tond and exercises it on real x86_64 and ARM64 servers.
