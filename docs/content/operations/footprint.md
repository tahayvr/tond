# Resource footprint

Tond is built to leave almost all of a small server to your apps. This page lists what Tond itself uses in memory, CPU and disk, the limits that keep it and your builds in check, and the server size to plan for.

## Memory and CPU at idle

Tond is two processes. Their idle budgets are enforced by the resource test (`e2e/budget.sh`) that runs against a fresh install:

| Resident memory | 0 apps | 50 apps |
| --- | --- | --- |
| `tond-server` | 15 MB or less | 40 MB or less |
| `tond-proxy` | 12 MB or less | 25 MB or less |

At idle, the test also holds Tond to these limits:

| Measurement | Budget |
| --- | --- |
| CPU, both processes together | 0.2 % of one core |
| Background wakeups per minute | 6 for `tond-server`, 2 for `tond-proxy` |
| SQL statements per minute by `tond-server` | 10, none a full table scan |
| Journal lines per hour, per service | 10 |

With traffic (20 apps, about 5 requests per second spread over them), the budgets are: `tond-server` grows by at most 10 MB over idle, `tond-proxy` by at most 2 MB over a TLS-warmed idle, and both together use at most 0.5 % of one core.

Tond stays this quiet because background work sleeps until its next deadline or until the data it watches changes, instead of polling on a short timer. The proxy's routing table is pushed to it when something changes, with a 60-second fallback check.

> [!NOTE]
> These are budgets, not guarantees for every workload. Deploys, snapshots and many open log streams use more while they run. The System page shows the live numbers (see [Observability](/operations/observability)).

## Service limits

Both units set systemd resource limits, so a bug in Tond cannot take the server down:

| | `tond-server` | `tond-proxy` |
| --- | --- | --- |
| `MemoryHigh` (throttled above) | 512 MB | 192 MB |
| `MemoryMax` (hard cap) | 1 GB | 384 MB |
| `TasksMax` | 512 | 128 |
| `OOMScoreAdjust` | -900 | -900 |
| `MALLOC_ARENA_MAX` | 2 | 2 |

The low OOM score means that under memory pressure the kernel kills app containers first, not the proxy (without which no app is reachable) or the server. Child processes that `tond-server` starts, such as `git`, the Docker CLI and `tar`, are reset to a neutral OOM score so they cannot hide behind that protection.

`MALLOC_ARENA_MAX=2` stops glibc from creating up to eight malloc arenas per core, which keeps resident memory flat.

`tond-server`'s cgroup also holds Git clones, `docker compose` calls and host-terminal shells. Image builds do not run there.

## Builds

Image builds run with `docker build --cgroup-parent tondbuild.slice`, so every build step lands in one systemd slice with its own limits:

```ini
[Slice]
MemoryHigh=50%
MemoryMax=60%
CPUWeight=50
IOWeight=50
```

Together, all builds may use at most 60 % of RAM. A runaway `npm install` is killed inside its build instead of pushing the server into an out-of-memory state that takes out running apps. Builds also get half the CPU and I/O weight of other work, so running apps stay responsive. Only one build runs at a time by default (`TOND_BUILD_CONCURRENCY`, see [Configuration](/reference/configuration)).

To change the slice limits:

```sh
sudo systemctl edit tondbuild.slice
```

## App containers

Every container Tond starts gets a memory limit. If you set none, the default is 75 % of the server's RAM (at least 256 MB), so one runaway container cannot starve Tond, the proxy and `sshd`. Each container is also limited to 4096 processes and threads.

Container logs use Docker's `json-file` driver with rotation: at most three files of 10 MB per container. The installer also writes the same rotation into `/etc/docker/daemon.json` as the default for containers Tond did not start, unless you configured a different log driver.

## Docker

Docker Engine (`dockerd` and `containerd`) runs next to Tond and is not part of Tond's budgets above. Its own memory use, images, build cache and container layers count against the server like any other Docker host. The System page's Disk tab shows how much of the disk Docker uses.

## Disk

Everything Tond keeps lives in `/opt/tond`:

| Path | Contents |
| --- | --- |
| `/opt/tond/bin` | `tond-server`, `tond-proxy`, `tond`, `update.sh` |
| `/opt/tond/dashboard` | The built dashboard |
| `/opt/tond/data/tond.db` (with `-wal`, `-shm`) | The SQLite database: settings, deploy logs, metrics, audit log |
| `/opt/tond/data/tond.env` | `TOND_SECRET_KEY` |
| `/opt/tond/data/volumes/` | App, cron and compose volumes |
| `/opt/tond/data/databases/` | Managed database data |
| `/opt/tond/data/backups/snapshots/` | Local snapshots |
| `/opt/tond/data/builds/` | Build working directories |
| `/opt/tond/data/compose/` | Compose files written for deploys |
| `/opt/tond/data/uploads/` | Uploads in progress |
| `/opt/tond/data/keys/` | Deploy keys |
| `/opt/tond/data/certs/`, `acme_account.json` | TLS certificates and the ACME account (owned by `tond-proxy`) |
| `/opt/tond/data/access/` | Opt-in per-app access logs, at most 60 MB per app |
| `/opt/tond/data/docker-config/` | Docker CLI configuration |

The database grows with deploy logs, metrics and history, and is bounded by the retention rules. Settings → Data retention shows the database size and the largest tables. See [Retention](/operations/retention).

Docker images are pruned after each successful deploy: Tond keeps the images of the three newest successful deployments per app (for rollback) plus the running one. The System page's Disk tab has a **Free up space** action that removes unused build cache and dangling images only.

### Journal

The installer caps the systemd journal at 500 MB with a drop-in, `/etc/systemd/journald.conf.d/tond.conf`, unless `SystemMaxUse` is already set somewhere. Your own setting always wins. Delete the drop-in or set `SystemMaxUse` elsewhere to change it.

## Recommended server size

The installer checks the server before it installs and warns when it is small:

| Resource | Minimum | Recommended |
| --- | --- | --- |
| Memory | 2 GB | 4 GB |
| Free disk on `/` | | 20 GB |
| CPU architecture | x86_64 or aarch64 | |

Below 2 GB of RAM the installer still runs, but warns. Most of the memory and disk is for your apps, their images and builds, not for Tond. See [Supported systems](/support-matrix) for the full matrix.
