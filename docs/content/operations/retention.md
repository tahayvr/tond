# Retention

Tond keeps logs, metrics and history in its SQLite database, and snapshots and images on disk. Everything that would otherwise grow without bound has a retention rule. This page lists how long each kind of data is kept and where to change it.

## How cleanup runs

A background job, the janitor, starts two minutes after `tond-server` starts and then runs every hour. It deletes in batches of 2,000 rows with a short pause between batches, so it never holds the database's write lock for long (`tond-proxy` reads the same file). After each pass it refreshes SQLite's query planner statistics.

Retention values come from the `settings` table. A missing or unreadable value falls back to the default.

## Configurable rules

Change these in the dashboard under **Settings → Data retention**, or with `PUT /api/v1/settings/retention` (see [HTTP API](/reference/api/)). Changing them needs the `settings:write` permission, and values outside the range are refused.

| Setting | Default | Range | What it removes |
| --- | --- | --- | --- |
| `retention_deploy_logs_deployments` | 30 deploys | 1 to 500 | Deploy logs beyond the logs of an app's last N deployments. |
| `retention_deploy_logs_days` | 90 days | 1 to 365 | Deploy logs older than N days. The logs of an app's most recent deployment are always kept. |
| `retention_deployments_per_app` | 50 deploys | 30 to 1000 | Deployment records beyond the last N per app. Queued and running deployments, and the recent successful ones that can still be rolled back to, are never deleted. |
| `retention_cron_runs_per_job` | 100 runs | 10 to 1000 | Cron runs beyond the last N per job. |
| `retention_cron_runs_days` | 30 days | 1 to 365 | Cron runs older than N days. The latest run of each job is always kept, and running rows are never deleted. |
| `retention_audit_log_days` | 180 days | 30 to 3650 | Audit log entries older than N days. See below. |
| `retention_app_events_days` | 90 days | 7 to 365 | Restart, crash and out-of-memory events. |
| `retention_alert_history_days` | 90 days | 7 to 365 | Alert episodes resolved more than N days ago, with their notification deliveries. |
| `retention_uptime_days` | 90 days | 7 to 365 | Hourly uptime rollups and ended incidents. |
| `retention_request_metrics_days` | 90 days | 7 to 365 | Hourly request metrics per app and preview. |
| `retention_host_history_days` | 365 days | 30 to 1095 | Hourly host history (CPU, memory, disk). |

The same form shows the database size, the size of its write-ahead log, free pages, and the largest tables, so you can see what a rule will save.

## Fixed tiers

Fine-grained samples are kept for a short, fixed time. Only the coarser rollups follow the settings above.

| Data | Kept for |
| --- | --- |
| Container metrics, raw samples | 25 hours |
| Container metrics, 5-minute rollups | 8 days |
| Container metrics, hourly rollups | 90 days |
| Host history, 1-minute rows | 48 hours |
| Uptime results, 1-minute rows | 48 hours |
| Request metrics, minute rows | 48 hours |

Metrics of apps, databases and cron jobs that no longer exist are removed as well. See [Metrics, uptime and alerts](/concepts/observability).

## Short-lived records

These have fixed lifetimes and are removed on every pass:

- Expired dashboard sessions, and sessions idle past the idle timeout (`session_idle_timeout_hours`, see [Configuration](/reference/configuration)).
- Pending 2FA logins, password links, terminal tickets and GitHub App setup flows once they expire.
- Webhook replay IDs older than the 72-hour replay window.
- Preview deployment records a week after the preview expired or failed.
- Docker networks labelled by Tond that no container has used for over an hour.

## Audit log

The audit log is hash-chained: each entry includes the hash of the one before it, so an edited or deleted entry breaks verification. Retention keeps that property. When entries older than `retention_audit_log_days` are removed, Tond deletes a prefix of the chain in one transaction, records the last removed entry's sequence number and hash as a checkpoint, and appends an `audit_pruned` entry naming it. Verification (`tond audit verify`) starts from the checkpoint, so the retained chain still verifies and the pruning itself is on the record.

If you ship the audit log to a syslog server or webhook, entries that retention removes before they were delivered are counted as dropped. Keep the retention longer than any expected shipping outage. See [Audit log](/concepts/audit-log).

## Snapshots

Snapshot retention is part of the backup configuration, not the settings table. Change it on the Snapshots page or with `tond snapshot config set` (see [CLI](/reference/cli)).

| Setting | Default | Meaning |
| --- | --- | --- |
| Full snapshots to keep | 7 | The newest N completed full snapshots are kept. The same count applies per database to database backups. |
| Pre-deploy snapshots per app | 10 | The newest N completed pre-deploy snapshots of each app are kept. |
| Keep local copy | on | When a remote storage provider is set, whether the local copy stays after upload. |

Older snapshots are deleted after each new one completes, both the local directory and the remote copy. See [Snapshots and backups](/concepts/backups).

## Images for rollback

After each successful deploy or rollback, Tond prunes the app's older images. It keeps the images of the three newest successful deployments plus whatever is running. Deployments whose image was pruned can no longer be rolled back to, and the dashboard shows that. See [Deploys and rollback](/concepts/deploys).

## Logs outside the database

- Container logs: Docker's `json-file` driver, three files of 10 MB per container.
- Per-app access logs (opt-in): at most 60 MB per app.
- The systemd journal, where `tond-server` and `tond-proxy` log: capped at 500 MB by the installer unless you set your own limit. See [Resource footprint](/operations/footprint).
