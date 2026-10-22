# Configuration

Tond has three layers of configuration: environment variables read by the processes at start, server settings stored in the database and edited from the dashboard or the CLI, and flags for the installer. This page lists all three.

## Environment variables

`tond-server` and `tond-proxy` run as systemd services. Their environment comes from the `Environment=` lines in `/etc/systemd/system/tond-server.service` and `/etc/systemd/system/tond-proxy.service`, from drop-in files, and (for `tond-server` only) from `/opt/tond/data/tond.env`.

### tond-server

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOND_SECRET_KEY` | none (required) | Key for encrypting stored secrets at rest. Release builds refuse to start without it. Set in `/opt/tond/data/tond.env`. |
| `TOND_PREVIOUS_SECRET_KEY` | unset | The old key during a key rotation. Values only the old key can read are re-encrypted with `TOND_SECRET_KEY` at startup. Set in `/opt/tond/data/tond.env`. |
| `TOND_BIND` | `127.0.0.1` | Address the HTTP API listens on. |
| `TOND_PORT` | `3001` | Port the HTTP API listens on. |
| `TOND_DB_PATH` | `/opt/tond/data/tond.db` | SQLite database file. |
| `TOND_DATA_DIR` | `/opt/tond/data` | Data directory (volumes, builds, keys, backups). Not set in the unit; the default applies. |
| `TOND_SOCKET_PATH` | `tond.sock` next to the database | Root-only Unix socket the CLI uses. Requests on it need no credentials. |
| `TOND_PROXY_SOCKET` | `proxy.sock` next to the database | Control socket shared with `tond-proxy`. |
| `TOND_DASHBOARD_DIR` | `./dashboard` (the unit sets `/opt/tond/dashboard`) | Built dashboard files. Without an `index.html` there, the server runs API-only. |
| `TOND_DASHBOARD_DOMAIN` | unset | Seeds the `dashboard_domain` setting at startup while that setting is empty. After that the setting wins. The installer writes it to a drop-in (`tond-server.service.d/dashboard-domain.conf`). |
| `TOND_TERMINAL_USER` | `tond` | Unprivileged Linux user the host terminal runs as. |
| `TOND_BUILD_CONCURRENCY` | `1` | Number of image builds that may run at the same time. |
| `TOND_UPDATE_RELEASES_URL` | GitHub's releases API for `tahayvr/tond` | Where the daily update check lists releases. A `file://` path works too. See [The update check](#the-update-check). |
| `TOND_BUILD_CGROUP_PARENT` | `tondbuild.slice` | Cgroup parent passed to `docker build`, which puts every build under the memory cap of `tondbuild.slice`. An empty value disables it. |
| `TOND_SHUTDOWN_GRACE_SECS` | `45` | How long in-flight deploys, cron runs and snapshots may run after SIGTERM before they are marked interrupted. |
| `TOND_LOG_FORMAT` | `text` | `json` writes one JSON object per log line. Anything else is text. |
| `TOND_DNS_PROPAGATION_SECS` | `30` (`60` for Route 53) | Seconds tond-server waits after publishing an ACME DNS-01 record through a DNS provider, before tond-proxy lets the CA check it. At most 600. See [Domains and certificates](/concepts/domains#dns-providers-and-wildcard-certificates). |
| `RUST_LOG` | `info` | Log level filter (`trace`, `debug`, `info`, `warn`, `error`, or per-module directives). |
| `DOCKER_CONFIG` | unit sets `/opt/tond/data/docker-config` | Docker CLI config directory. It must be writable; `/root` is read-only under the service sandbox. |
| `DOCKER_HOST` | Docker's default socket | Docker daemon address, honoured by the Docker client library and passed to `docker compose`. |
| `MALLOC_ARENA_MAX` | unit sets `2` | Limits glibc malloc arenas to keep memory use flat. |

Some variables exist for development and the end-to-end tests. You should not need them on a production server:

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOND_CGROUP_ROOT` | `/sys/fs/cgroup` | Where the metrics collector reads cgroup files. |
| `TOND_PROC_ROOT` | `/proc` | Where the metrics collector reads process files. |
| `TOND_UPTIME_RESOLVE` | unset | Address uptime probes connect to instead of resolving the domain. |
| `TOND_UPTIME_CA_FILE` | unset | Extra PEM certificates uptime probes trust. |
| `TOND_DNS_TEST_PROVIDER` | unset | `1` or `true` allows the test-only `challtestsrv` DNS provider kind (Pebble's challenge test server), used by the end-to-end test. Debug builds always allow it. The installer never sets it. |

### tond-proxy

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOND_DB_PATH` | `/opt/tond/data/tond.db` | Database file, opened read-only. |
| `TOND_DATA_DIR` | `/opt/tond/data` | Where certificates (`certs/`), the ACME account and access logs live. |
| `TOND_SERVER_ADDR` | `127.0.0.1:3001` | Address of `tond-server` for dashboard and API traffic. Keep it in step with `TOND_BIND` and `TOND_PORT`. |
| `TOND_PROXY_SOCKET` | `proxy.sock` in `TOND_DATA_DIR` | Control socket shared with `tond-server`. |
| `TOND_ACME_STAGING` | `false` | `1` or `true` uses the Let's Encrypt staging CA. Staging certificates are not trusted by browsers. |
| `TOND_ACME_DIRECTORY` | unset | Custom ACME directory URL. Overrides `TOND_ACME_STAGING`. |
| `TOND_LOG_FORMAT` | `text` | `json` for one JSON object per line. |
| `RUST_LOG` | `info` | Log level filter. |
| `MALLOC_ARENA_MAX` | unit sets `2` | Limits glibc malloc arenas. |

The proxy reads the dashboard, apps and preview domains, and the zones of configured DNS providers (never their tokens), from the database, not from its environment.

### tond CLI

| Variable | Flag | Default | Meaning |
| --- | --- | --- | --- |
| `TOND_SERVER` | `--server` | unset | Server URL to use over HTTP(S) instead of the local socket. Needs a token. |
| `TOND_TOKEN` | `--token` | unset | API token (`tond_pat_…`) for `--server` connections. |
| `TOND_SOCKET_PATH` | `--socket` | `/opt/tond/data/tond.sock` | Local socket used when no server URL is set. |
| `TOND_IDEMPOTENCY_KEY` | `--idempotency-key` | a new UUID per request | Idempotency key sent with mutating requests: a repeated command with the same key returns the first result instead of acting twice. See [Error codes](/reference/error-codes#retries-and-idempotency). |
| `TOND_ALLOW_UNSIGNED` | none | unset | `1` lets `tond update --direct` install a release that has no signature. A signature that fails to verify is always refused. |
| `TOND_RELEASE_BASE` | none | `/opt/tond/RELEASE_BASE`, else `https://github.com/tahayvr/tond/releases` | Where `tond update --direct` downloads releases from (see [Release downloads](#release-downloads)). |

See [CLI](/reference/cli) for every command and flag.

### Changing a variable

Do not edit the unit files in `/etc/systemd/system` directly: the installer and updates replace them. Use a drop-in instead:

```sh
sudo systemctl edit tond-server
```

Add the variables under a `[Service]` section, save, and restart:

```ini
[Service]
Environment=TOND_LOG_FORMAT=json
Environment=TOND_BUILD_CONCURRENCY=2
```

```sh
sudo systemctl restart tond-server
```

The same works for `tond-proxy`. To check what a service actually sees:

```sh
systemctl show tond-server -p Environment
```

### The secret key file

`TOND_SECRET_KEY` lives in `/opt/tond/data/tond.env`, owned by root with mode `0600`, so it never appears in the unit files or in `systemctl show`. The installer generates it on first install and never replaces an existing key.

```sh
TOND_SECRET_KEY=<64 hex characters>
```

> [!WARNING]
> Back up `tond.env`. Without it, stored secrets (environment variables, registry passwords, Git tokens, storage keys, 2FA secrets) and remote snapshot copies cannot be decrypted.

To rotate the key:

1. In `tond.env`, rename the current line to `TOND_PREVIOUS_SECRET_KEY=<old key>`.
2. Add a new `TOND_SECRET_KEY=<new key>`, for example from `openssl rand -hex 32`.
3. Restart `tond-server`. It re-encrypts every value at startup.
4. Remove the `TOND_PREVIOUS_SECRET_KEY` line and restart again.

See [Environment and secrets](/concepts/secrets) for what the key protects.

## Server settings

Server-wide settings are rows in the `settings` table of `tond.db`. Change them in the dashboard under **Settings**, with `tond settings …` on the server (see [CLI](/reference/cli)), or through the HTTP API (`/api/v1/settings`, `/api/v1/settings/retention`; see [HTTP API](/reference/api/)). Writing settings needs the `settings:write` permission.

### General

| Key | Default | Where to change | Meaning |
| --- | --- | --- | --- |
| `server_name` | `Tond Server` | Settings, `tond settings set-server-name` | Display name of this server. |
| `dashboard_domain` | empty (the installer sets one) | Settings, `tond settings set-dashboard-domain` | Domain the dashboard and API are served on. Saving it requests a certificate. |
| `apps_domain` | empty (the installer suggests one) | Settings, `tond settings set-apps-domain` | Apps get `<app>.<apps_domain>`. Must be a different site from the dashboard and preview domains, with a wildcard DNS record. Empty means apps need a custom domain. |
| `preview_domain` | empty | Settings, `tond settings set-preview-domain` | Previews are served as `<id>-<app>.<preview_domain>`. Must be a separate registrable domain. Empty means previews are disabled. |
| `upload_max_bytes` | 1 GiB | Settings (upload limit) | Largest unpacked source archive for uploads (`tond apps push`). Accepted range: 1 MiB to 64 GiB. |
| `session_idle_timeout_hours` | `12` | Settings → Dashboard sessions | A dashboard session ends after this many hours without a request. Range 1 to 720. |
| `session_max_age_days` | `30` | Settings → Dashboard sessions | A dashboard session ends this many days after sign-in, however active. Range 1 to 365. |
| `host_terminal` | off | `sudo tond settings host-terminal on` / `off` | Enables the browser shell on the host. Can only be changed on the server itself, never from the dashboard or with a token. See [Host terminal](/guides/host-terminal). |
| `alert_quiet_hours` | disabled, 22:00 to 07:00 UTC, critical alerts bypass | Alerts page | Global window in which notifications wait. See [Alerts and heartbeat](/guides/alerts). |

See [Domains and certificates](/concepts/domains) and [Previews](/concepts/previews) for the domain rules.

### Retention

Each retention rule is a settings key. Change them in the dashboard under **Settings → Data retention**, or with `PUT /api/v1/settings/retention`. Values outside the range are refused.

| Key | Default | Range | Unit |
| --- | --- | --- | --- |
| `retention_deploy_logs_deployments` | 30 | 1 to 500 | deploys per app |
| `retention_deploy_logs_days` | 90 | 1 to 365 | days |
| `retention_deployments_per_app` | 50 | 30 to 1000 | deploys per app |
| `retention_cron_runs_per_job` | 100 | 10 to 1000 | runs per job |
| `retention_cron_runs_days` | 30 | 1 to 365 | days |
| `retention_audit_log_days` | 180 | 30 to 3650 | days |
| `retention_app_events_days` | 90 | 7 to 365 | days |
| `retention_alert_history_days` | 90 | 7 to 365 | days |
| `retention_uptime_days` | 90 | 7 to 365 | days |
| `retention_request_metrics_days` | 90 | 7 to 365 | days |
| `retention_host_history_days` | 365 | 30 to 1095 | days |

[Retention](/operations/retention) explains what each rule removes and what is kept regardless.

### Other configuration tables

A few groups of settings have their own tables instead of `settings` rows:

- **Snapshots** (`backup_config`): schedule, how many snapshots to keep, pre-deploy snapshots, remote storage. Change them on the Snapshots page or with `tond snapshot config set`. See [Snapshots and backups](/concepts/backups).
- **Database backup schedules** (columns of `databases`): each database's own backup schedule and how many scheduled backups to keep. Change them on the database's Backups tab or with `PUT /api/v1/databases/{id}/backup-schedule`. See [Databases](/concepts/databases#backup-schedule).
- **DNS providers** (`dns_providers`): Integrations → DNS providers, or `/api/v1/dns-providers`. Tokens are encrypted with `TOND_SECRET_KEY`. See [Domains and certificates](/concepts/domains#dns-providers-and-wildcard-certificates).
- **Git forge tokens** (`gitlab_tokens`, `gitea_tokens`): Integrations → GitLab and Integrations → Gitea / Forgejo. See [Deploy from Git](/guides/deploy-from-git).
- **Heartbeat** (`heartbeat_config`): Settings → Heartbeat. See [Alerts and heartbeat](/guides/alerts).
- **Audit log shipping** (`audit_shipping`): Settings → Audit log shipping. See [Audit log](/concepts/audit-log).

## install.sh flags

```sh
curl -fsSL https://tond.io/install | sudo bash -s -- [flags]
```

| Flag | Meaning |
| --- | --- |
| `--non-interactive`, `-y` | Skip all prompts and use safe defaults. Enables UFW when it is installed but inactive, and uses the suggested domains. |
| `--dashboard-domain <domain>` | Dashboard domain. Default: `<server-ip>.sslip.io` (dots replaced by dashes) when the server has a public IPv4 address. |
| `--apps-domain <domain>` | Domain apps are served under (`<app>.<domain>`). Must be on a different site from the dashboard and preview domains, with a wildcard DNS record. Default: `<server-ip>.nip.io`, or `.sslip.io` when the dashboard is on nip.io. Pass `""` for none. |
| `--preview-domain <domain>` | Domain previews are served under (`<id>-<app>.<domain>`). Must be a separate domain you own, with a wildcard DNS record `*.<domain>`. Previews stay disabled until one is set. |
| `--allow-unsupported-os` | Install on a Linux distribution outside the support matrix. The installer prints a warning and continues. See [Supported systems](/support-matrix). |
| `--version <version>` | Release to download when the installer runs on its own (download mode). Default: the latest release. Also `TOND_VERSION`. |
| `--release-base <url>` | Where releases are downloaded from. Also `TOND_RELEASE_BASE`. See [Release downloads](#release-downloads). |
| `--help`, `-h` | Print usage and exit. |

An unknown flag stops the installer with an error. Without a flag, interactive runs ask for each domain.

The installer also reads these variables from its environment: `TOND_ALLOW_UNSIGNED=1` lets it install a downloaded release that has no signature file (a signature that does not verify is always fatal), `TOND_ALLOW_UNSUPPORTED_OS=1` is the same as `--allow-unsupported-os`, and `TOND_VERSION` and `TOND_RELEASE_BASE` are the same as `--version` and `--release-base`. The one-line installer passes them on, for example `curl -fsSL https://tond.io/install | sudo TOND_VERSION=0.2.0 bash`.

## update.sh flags

`update.sh` ships in every release bundle and is installed as `/opt/tond/bin/update.sh`. You rarely call it yourself: the dashboard (System → Updates) and `tond update` run it for you after taking a snapshot. See [Upgrading Tond](/guides/upgrading).

```sh
sudo /opt/tond/bin/update.sh [flags]
```

| Flag | Meaning |
| --- | --- |
| `--version <version>` | Update to this release, without the `v` prefix. Default: the latest release. |
| `--bundle <dir>` | Install an already extracted, already verified release bundle instead of downloading one. `tond update` uses this after verifying the download. |
| `--rollback` | Go back to the version before the last update, from that update's backup. Only within 7 days, and only once per update. |
| `--backup <dir>` | With `--rollback`: the update backup (`/opt/tond/data/backups/tond_backup_<time>`) to roll back to. Default: the newest one that fits. |
| `--force` | Reinstall even when that version is already installed. |
| `--dry-run` | Say what would happen without changing anything. |
| `--allow-unsupported-os` | Run on a distribution outside the [support matrix](/support-matrix). Not needed when the install was made with this flag. |
| `--release-base <url>` | Download releases from this base. Also `TOND_RELEASE_BASE`. See [Release downloads](#release-downloads). |
| `--help`, `-h` | Print usage and exit. |

It reads these variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOND_PORT` | `3001` | Port tond-server listens on, for the health check. |
| `TOND_UPDATE_HEALTH_TIMEOUT` | `90` | Seconds the new tond-server gets to become healthy before the update is rolled back. |
| `TOND_ALLOW_UNSIGNED` | unset | `1` accepts a downloaded release without a signature file. |
| `TOND_RELEASE_BASE` | `/opt/tond/RELEASE_BASE`, else GitHub releases | Where releases are downloaded from. |

### The update check

The daily check for new releases is on by default. Turn it off under System → Updates (or `PUT /api/v1/system/update/settings` with `{"check_enabled": false}`). The check reads the release list from the [release base](#release-downloads): GitHub's releases API for a GitHub base, or `<base>/releases.json` (the same format) for any other base. `TOND_UPDATE_RELEASES_URL` overrides that URL; a `file://` path works too (the end-to-end test uses one).

### Release downloads

Releases are downloaded from `https://github.com/tahayvr/tond/releases` by default. Every downloader (the installer, `update.sh`, `tond update` and the one-line scripts) uses the same layout:

- `<base>/download/v<version>/tond-linux-<arch>.tar.gz`, with `.sha256` and `.minisig` next to it
- `<base>/latest` for the latest release. GitHub redirects it to `…/releases/tag/v<version>`; a plain file server can serve a file whose first line is the tag.

Set `TOND_RELEASE_BASE` (or `--release-base`) to use a mirror. The installer records a non-default base in `/opt/tond/RELEASE_BASE`, so later updates and the update check use it too. Delete that file to go back to GitHub. A mirror can't weaken verification: every download is still checked against its checksum and the minisign signature from the release key built into Tond.
