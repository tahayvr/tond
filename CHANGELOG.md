# Changelog

All notable changes to Tond. Versions follow [SemVer](https://semver.org/): while
Tond is on 0.x a minor release may contain breaking changes, which are marked
**Breaking**. Release tags are `v<version>`.

## 0.2.0 — 2026-10-21

The first launch-track release: everything built since v0.1.0 (Phases 0 to 3).
There is no upgrade path from v0.1.0. The database schema was reset to one
baseline, so install 0.2.0 fresh.

### Install, updates and releases

- Signed releases. Both tarballs and their checksums are signed with minisign,
  and every tarball carries SLSA build provenance. `install.sh`, `update.sh`,
  `tond.io/install`, `tond.io/update` and `tond update` refuse a release that
  doesn't verify, and refuse unsigned releases unless `TOND_ALLOW_UNSIGNED=1`.
- Builds for x86_64 and ARM64 (aarch64), both end-to-end tested.
- A support matrix: Debian 12 and 13, Ubuntu 24.04 and 26.04. Other systems are
  refused unless you pass `--allow-unsupported-os`. Docker Engine 25 or newer is
  required, and Docker is installed from Docker's apt repository.
- In-place updates from the dashboard (System → Updates) or with `tond update`:
  a full snapshot is taken first, then the release's own `update.sh` swaps
  binaries, runs migrations, and rolls back by itself when the new version
  doesn't come up healthy. You can roll back by hand for 7 days. A daily
  update check can be turned off.
- `TOND_RELEASE_BASE` (or `--release-base`) points installs and updates at
  another release location. `TOND_VERSION` (or `--version`) pins the release
  that `install.sh` downloads.
- The installer defaults the dashboard to `<server-ip>.sslip.io` and apps to
  `<server-ip>.nip.io`, so HTTPS works immediately. It caps the journal at
  500 MB, sets Docker log rotation, and adds an SSH-safe firewall.

### Security

- **Breaking**: every stored secret is encrypted with `TOND_SECRET_KEY` and is
  write-only in the API. Admins can reveal values on purpose, and the reveal is
  audited.
- **Breaking**: Tond no longer trusts loopback TCP. The CLI talks to a root-only
  Unix socket; tond-proxy forwards internet traffic over loopback, so requests
  arriving there are not trusted.
- tond-proxy runs as an unprivileged user with read-only database access, and
  `/opt/tond/data` is locked down. Both services run with systemd hardening.
- **Breaking**: first-run setup uses a one-time token. Sessions use
  `__Host-` cookies with a CSRF header and Origin checks. Logins are throttled,
  TOTP codes can't be replayed, and 2FA backup codes persist.
- **Breaking**: webhooks must be signed and are protected against replay and
  rate-limited. GitHub App flows are bound to the admin who started them.
- **Breaking**: users have roles and scoped API tokens (`tond token`, with app
  filters, allowed IPs and expiry). The audit log is hash-chained, can be
  verified with `tond audit verify`, and can be shipped to syslog or a signed
  webhook.
- Per-app Docker networks. Previews don't get the app's secrets or production
  volumes, and are served only on a separate preview domain. Bind mounts are
  confined, containers are hardened, and published ports bind to loopback.
- The host terminal is off by default and uses step-up auth with single-use
  tickets. Secrets are scrubbed from container logs. Git tokens stay out of
  URLs, logs and images.

### Deploys and apps

- A build queue: one deploy per app, a newer deploy supersedes a queued one,
  and a global build cap applies. Deploys record their phases, commit, trigger
  and a likely cause of failure, and show success rate and median duration.
- Images are tagged per deployment. A rollback restores the exact image that
  ran before.
- Per-app readiness: HTTP, TCP or background worker. Background workers and
  TCP services are supported.
- Graceful shutdown, plus startup reconciliation of whatever a crash left
  behind.
- Compose apps: build, pin and roll back each service; route domains per
  service; resolve `${VAR}` from the app's environment; snapshot and restore
  named volumes; keep routes up while a deploy runs.
- A one-click catalog of compose templates (Gotify, Audiobookshelf, listmonk,
  Stirling-PDF and more). The catalog shows memory estimates and refuses
  templates that have no image for the server's CPU.
- Several domains per app, with aliases and redirects. Wildcard certificates
  through DNS-01 (Cloudflare, Hetzner DNS, DigitalOcean, Route 53), uploaded
  certificates, and a separate apps domain for automatic hostnames.
- Project env vars are injected into member apps, previews, compose services
  and cron jobs.
- Previews for GitLab, Gitea and Forgejo. Deploys report commit statuses, and
  merge requests get comments.

### Databases and backups

- Scheduled per-database backups, restore into a new database, versions and
  variants, upgrades, an IP allowlist and TLS. Managed Redis and KeyDB require
  auth.
- **Breaking**: snapshots are streamed as encrypted, authenticated archives.
  Pre-deploy snapshots can be turned off, and retention never deletes the
  newest snapshot.

### Observability

- Container metrics from cgroups, stored compactly with rollups. The proxy
  reports request metrics, and access logs are opt-in.
- **Breaking**: a real alert engine with pending, firing and resolved states,
  snooze and quiet hours. Every alert rule notifies someone.
- Uptime probes, heartbeat monitors, host history, Docker disk usage, and an
  events feed for each app.
- JSON logs. An authenticated `/metrics` endpoint. Retention settings for logs,
  deployments, runs, audit entries, metrics and sessions.
- Tond measures itself: process stats, background-loop health, and idle
  resource budgets checked in the end-to-end test.
- Lower idle load with many apps and alert rules: metrics are written one
  statement per flush, retention runs one query per rule instead of per app,
  alert evaluation re-reads rules, apps and certificates only when they change,
  and uptime probes reuse one TLS setup.

### Routing

- Event-driven routing: the proxy picks up changes immediately, and a deploy
  stops the old container only after the proxy confirms the new route.
- tond-proxy restarts in seconds and listens on IPv6 when the host has it.
  ACME uses per-domain backoff and checks DNS before ordering a certificate.

### API and CLI

- **Breaking**: the API is versioned under `/api/v1`, returns structured
  errors with stable codes, and publishes an OpenAPI 3.1 spec
  (`/api/v1/openapi.json`). Mutations accept `Idempotency-Key`.
- **Breaking**: the CLI is automation-ready: `--json`, documented exit codes,
  `--yes` for destructive commands, `deploy --wait`, and `--token` instead of
  `--api-key`. New commands include `tond apps domains`, `tond apps services`,
  `tond templates`, `tond token`, `tond users`, `tond audit` and `tond update`.

### Dashboard

- One system for status, feedback and confirmations, with WCAG AA colours in
  light and dark mode. Tabs live in the URL. Layouts work on phones. A command
  palette and keyboard shortcuts.
- New pages and views: live deploy view, unified log viewer, metrics charts,
  certificates, DNS providers, alerts, uptime, users and API tokens, the audit
  log, System → Updates, an API reference, the template catalog, and a
  first-run checklist.

## 0.1.0

Initial release (unsigned).
