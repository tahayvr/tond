# Isolation

Tond separates the parts of a server from each other so that a problem in one does not spread: apps from the host, apps from each other, the internet-facing proxy from everything else, and builds from running apps. This page lists each boundary and exactly what enforces it.

## App containers

Every container Tond starts, whether an app, a managed database, a cron job, a preview or a compose service, gets the same hardening:

| Setting | Value | Why |
| --- | --- | --- |
| `no-new-privileges` | on | setuid binaries and file capabilities cannot raise privileges after start. Entrypoints that start as root and drop privileges (`gosu`, `su-exec`) still work. |
| Dropped capabilities | `NET_RAW`, `MKNOD`, `AUDIT_WRITE`, `SETFCAP` | Removed from Docker's default set: no raw sockets for spoofing neighbours, no device nodes, no audit log writes, no setting file capabilities. |
| Process limit | 4096 processes and threads | Stops a fork bomb from taking the host down. |
| Memory limit | yours, or 75 % of RAM (at least 256 MB) | One runaway container cannot starve Tond, the proxy and `sshd`. |
| Logs | `json-file`, 3 files of 10 MB | A noisy container cannot fill the disk with logs. |
| Restart policy | `unless-stopped` | Containers come back after a reboot. |

Tond keeps the rest of Docker's default capability set on purpose. The official postgres, mysql, mongo and redis images change ownership of their data directory and drop to an unprivileged user, and nginx binds port 80 as root before dropping privileges. Dropping all capabilities would break them.

Root filesystems are not read-only, and user namespaces are not used: a process that runs as root inside a container is root on the shared kernel, limited by the settings above. See [Threat model](/security/threat-model) for what that means.

## Published ports

Containers publish their port on `127.0.0.1` only. `tond-proxy` reaches apps through those loopback ports, and nothing outside the server can.

This matters because Docker's iptables rules run before UFW: a port published on `0.0.0.0` is reachable from the internet whatever the firewall says. The only exception is a managed database whose public access an admin turned on explicitly. It binds to all interfaces.

## Networks

Nothing Tond starts joins Docker's default bridge, where every container can reach every other one. Each workload has a home network instead:

| Workload | Network | Who else is on it |
| --- | --- | --- |
| App in a project | `tond-<project id>` | The project's apps, databases and cron jobs |
| App without a project | `tond-net-app-<app>` | Databases linked to the app |
| Database without a project | `tond-net-db-<db>` | Nobody; container-to-container traffic is off |
| Cron job without a project | `tond-net-cron-<job id>` | Nobody; container-to-container traffic is off |
| Preview | `tond-net-preview-<slug>` | The other apps of the same project preview |

Docker isolates bridge networks from each other, so a container reaches only containers on its own networks. Linking a database to an app attaches the database to that app's network; two apps linked to the same database still cannot reach each other. Previews never join the project network, so pull-request code cannot reach production databases.

The installer also sets `icc: false` in `/etc/docker/daemon.json`, so containers left on Docker's default bridge by something other than Tond cannot reach each other either.

See [Networks and database links](/concepts/networks).

## Volumes

Users never choose host paths. They name a volume, and Tond maps it into a directory it owns:

| Workload | Host directory |
| --- | --- |
| App | `/opt/tond/data/volumes/<app>/<volume>` |
| Cron job | `/opt/tond/data/volumes/_cron/<job id>/<volume>` |
| Compose service | `/opt/tond/data/volumes/<app>/<relative path>` |

Volume names are validated when you configure them. Right before a container starts, Tond walks the path one component at a time from the volumes directory, refusing symlinks and anything that is not a directory, then checks that the resolved path is still inside. A container that plants a symlink in its volume to escape on the next deploy is caught.

See [Volumes](/concepts/volumes).

## Compose files

A compose file can ask for anything Docker can do. Tond parses every compose file before storing or deploying it, and refuses host-level options with a message naming the service and the option: privileged mode, the Docker socket, host namespaces, host devices, capabilities beyond Docker's default set, host bind mounts outside the app's volume directory, external networks and the default bridge. Variable substitution is refused in security-relevant fields.

The file handed to `docker compose` is rewritten: bind mounts point into the app's volume directory, published ports bind to `127.0.0.1`, and every service gets the same hardening as other containers. `docker compose` runs with a cleared environment, so `tond-server`'s own variables, including `TOND_SECRET_KEY`, cannot be interpolated into a container. Your stored compose file stays exactly as you wrote it.

## tond-proxy

The proxy is the only process that faces the internet, so it has the least privilege:

- It runs as the system user `tond-proxy`, with no home directory and no login shell.
- Its only capability is `CAP_NET_BIND_SERVICE`, to bind ports 80 and 443.
- It opens `tond.db` read-only, through the `tond-proxy` group. Anything it has to report (certificate results) goes to `tond-server` over its control socket, `proxy.sock`, which only root and `tond-proxy` may connect to.
- It cannot read `tond.env`, the CLI socket, deploy keys, backups or any other file in the data directory, and it cannot reach Docker.

Its systemd unit adds a strict sandbox: `ProtectSystem=strict` with only `/opt/tond/data` writable, `ProtectHome`, `PrivateTmp`, `PrivateDevices`, `ProtectProc=invisible`, `ProcSubset=pid`, kernel tunables, modules, logs, clock and hostname protected, `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`, `RestrictNamespaces`, `MemoryDenyWriteExecute`, `SystemCallFilter=@system-service` without `@privileged`, and `UMask=0077`.

## tond-server

`tond-server` runs as root: it drives Docker, which is root-equivalent anyway, owns the root-only CLI socket, and switches to the terminal user for host shells. systemd still confines it:

- `NoNewPrivileges=true`
- `ProtectSystem=strict` with only `/opt/tond/data` writable, and `ProtectHome=true`
- `PrivateTmp=true`: uploads and snapshot staging are invisible to other services and users
- `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectControlGroups`, `ProtectClock`, `ProtectHostname`
- `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK`, `RestrictNamespaces`, `RestrictRealtime`, `RestrictSUIDSGID`, `LockPersonality`, `KeyringMode=private`, `SystemCallArchitectures=native`

Child processes (`git`, the Docker CLI, `tar`) are started with a neutral OOM score, so they cannot shelter behind the server's protection from the OOM killer.

## The data directory

`/opt/tond/data` is `root:tond-proxy` with mode `3770`: the setgid bit gives new files the `tond-proxy` group, and the sticky bit lets the proxy create its own files but never remove or replace root's.

| Path | Owner and mode |
| --- | --- |
| `tond.db`, `tond.db-wal`, `tond.db-shm` | `root:tond-proxy 0640`: written by the server, read-only for the proxy |
| `proxy.sock` | `tond-proxy 0600` |
| `certs/`, `acme_account.json` | `tond-proxy` only (`0700` directories, `0600` files) |
| `access/` | `tond-proxy` only; the server reads it as root and refuses symlinks and hard links |
| `tond.env`, `setup-token`, `tond.sock` | root only (`0600`) |
| `builds/`, `compose/`, `volumes/`, `databases/`, `keys/`, `backups/`, `docker-config/` | root only (`0700`) |

The installer and updates reapply these permissions on every run.

## Builds

Image builds run with `docker build --cgroup-parent tondbuild.slice`, so every build step lands in one systemd slice. The slice caps all builds together at 60 % of RAM and gives them half the CPU and I/O weight of other work. A runaway build is killed inside its own cgroup instead of pushing running apps or Tond out of memory. Builds run one at a time by default.

Private repositories are cloned with credentials passed through Git's environment configuration, never in the URL, and deploy keys live in a root-only directory.

## The host terminal

The dashboard's host shell runs as the Linux user `tond` (`TOND_TERMINAL_USER`). The installer creates it with a locked password and removes it from the `docker`, `sudo`, `wheel` and `adm` groups if it finds it there, so the terminal is never a path to root. Admins who need root use SSH. The terminal is off by default and can only be switched on from the server. See [Host terminal](/guides/host-terminal).

## Secrets at rest

Stored secrets are encrypted in the database with AES-256-GCM under a key derived (HKDF-SHA256) from `TOND_SECRET_KEY`. Each value is bound to its purpose through the cipher's associated data, so a ciphertext copied into another column does not decrypt. This covers app, preview, project and cron environment variables, webhook secrets, managed database passwords, GitHub App credentials, GitLab tokens, notification secrets, registry passwords, storage provider keys, the heartbeat URL and users' TOTP secrets.

The key lives in `/opt/tond/data/tond.env` (root, `0600`), outside the database and outside the proxy's reach. Sessions, API tokens, setup tokens and password links are stored only as SHA-256 hashes. See [Environment and secrets](/concepts/secrets).
