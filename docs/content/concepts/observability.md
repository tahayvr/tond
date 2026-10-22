# Metrics, uptime and alerts

Tond measures what runs on your server and tells you when something breaks. It collects resource metrics for every container, request metrics from the proxy, host history and uptime checks, and it has an alert engine that notifies you over the channels you choose. Everything is stored in Tond's own SQLite database, so there is nothing extra to install.

## Resource metrics

One background task collects metrics for every app, database, cron job and preview:

- **Every 30 seconds** it reads each container's cgroup v2 files and network counters directly. It doesn't poll Docker for this, so sampling costs almost nothing.
- **What it records:** CPU, memory (and the memory limit), network received and sent, and the number of processes. Services of a Docker Compose app are added together.
- **Rollups:** raw samples are rolled up into 5-minute and 1-hour buckets, keeping the average and the peak.

| Tier | Resolution | Kept for |
| --- | --- | --- |
| Raw | 30 seconds | 25 hours |
| 5-minute | 5 minutes | 8 days |
| Hourly | 1 hour | 90 days |

See them on the **Metrics** tab of an app, database or cron job.

### Restart, crash and OOM events

The collector follows Docker's event stream and records:

- **restart**: a container exited on its own and was started again;
- **crash**: a container exited on its own and stayed down for a minute;
- **oom**: the kernel killed a container for going over its memory limit.

Exits that follow a stop or kill request, for example during a deploy or from the Stop button, are not crashes. Events are kept for 90 days by default. They show on the app's **Events** tab, and the **Out of memory** and **Crash loop** alert rules use them.

## Request metrics

tond-proxy counts every request per route: the dashboard, each app and each preview. Tond reads these counters every 30 seconds and stores, per minute:

- requests by status class (1xx to 5xx), with 502 and 504 counted separately;
- bytes in and out;
- latency histograms for the total time and for the time to the app's first response, from which Tond computes percentiles such as p95.

Per-minute rows are kept for 48 hours and hourly rows for 90 days by default. The **5xx error rate** and **Slow responses (p95)** alert rules read them.

## Access logs

Access logs are off by default. Turn them on per app on the app's **Logs** tab, under **Access log**. tond-proxy then writes one line per request:

```text
2026-09-25T12:00:00.123Z GET /path 200 12.3ms 1534 203.0.113.9
```

The fields are time, method, path, status, total duration, response size in bytes and client IP. Query strings are left out unless you also turn on **Include query strings**, because they often carry tokens. Each app's log is capped at 60 MB: at 30 MB the file rotates and the older half is replaced. Requests never wait for the disk: when the write queue is full, lines are dropped and counted.

You can search the log, download it as text or clear it. Reading needs `logs:read`, and changing the settings needs `apps:write`. The files are deleted when the app is deleted.

## Host history and disk forecast

The same collector samples the server itself: CPU and its peak, load averages, memory, swap, and disk space and inodes of both the data directory's filesystem and the root filesystem. It stores one row a minute (kept 48 hours) and one an hour (kept 365 days by default).

The **disk forecast** fits a straight line through the data filesystem's used space over the last 24 hours and the last 7 days. If usage is rising, it estimates when the disk will be full and reports the sooner of the two estimates. Growth under 16 MB a day, or a date more than a year away, counts as "not filling".

See both on the **System** page (**Overview**, **History**, **Disk** and **Uptime** tabs).

## Uptime checks

Uptime checks test your public URLs from the server, the way a visitor would reach them: DNS, then TCP to the public address, then TLS, then HTTP.

- Every app with a domain gets a check automatically, and so does the dashboard (`/health` on the dashboard domain).
- Checks of apps that are stopped, never deployed, or background workers are paused. Pausing closes any open incident.
- A failed probe is retried once, 3 seconds later, before it counts, so one dropped packet doesn't open an incident.
- A failure opens an **incident** with its cause, such as DNS, TLS or a wrong status. The incident closes when the check passes again.

| Setting | Default | Range |
| --- | --- | --- |
| Path | `/` | Must start with `/` |
| Interval | 1 minute | 1 to 60 minutes |
| Expected status | Any 2xx or 3xx | A status code from 100 to 599 |
| Timeout | 10 seconds | 1 to 30 seconds |

Configure an app's check on its **Uptime** tab (`apps:write`), and the dashboard's check on **System → Uptime** (`settings:write`). One-minute results are kept for 48 hours; hourly results and incidents for 90 days by default.

## Heartbeat

A server can't report its own outage. The heartbeat is a dead-man's switch: Tond pings a URL at an outside monitor such as Healthchecks.io, Uptime Kuma (push) or Better Stack, and that service alerts you when the pings stop.

Tond sends the ping only while it is healthy:

- Docker answers;
- tond-proxy is connected to tond-server;
- the metrics collector ran in the last two minutes;
- the database answers.

Otherwise it skips the ping and records why, and your monitor alerts after its grace period. Set it up in **Settings → Heartbeat**: the ping URL, `GET` or `POST`, and an interval from 1 minute to 1 day (default 5 minutes). The URL is stored encrypted, and afterwards only its host is shown. See [Alerts and heartbeat](/guides/alerts).

## Alert rules

An alert rule watches one condition and notifies channels when it holds. Create rules on the **Alerts** page (**Rules** tab).

| Rule | Watches | Default threshold | Default "for" |
| --- | --- | --- | --- |
| App down | App `failed` or `unhealthy`, or its uptime check down | — | 2 minutes |
| Crash loop | Restarts in the window | 3 in 10 minutes | Immediately |
| Out of memory | An OOM kill in the window | Window: 15 minutes | Immediately |
| CPU usage | CPU above the threshold | 80% | 5 minutes |
| Memory usage | Memory above the threshold, as a share of the limit | 90% | 5 minutes |
| 5xx error rate | Share of 5xx responses in the window | 5% over 5 minutes | 2 minutes |
| Slow responses (p95) | p95 latency in the window | 1000 ms over 5 minutes | 5 minutes |
| Deploy failed | A failed deployment | — | Immediately |
| Disk usage | Disk usage of the data filesystem | 90% | 5 minutes |
| Disk full forecast | Hours until the disk is full | 48 hours | 15 minutes |
| Automatic disk cleanup | Tond pruned images and build cache under disk pressure | — | Immediately |
| Certificate expiry | Days until a certificate expires | 14 days | Immediately |
| Backup failed | A failed snapshot | — | Immediately |
| Cron job failed | A failed cron run | — | Immediately |

App rules apply to one app or to every app. Each rule also has a **severity** (`info`, `warning` or `critical`) and a **repeat** interval: while an alert fires and nobody has acknowledged it, Tond notifies again every 4 hours by default. You can set it to never repeat.

### States

Each rule tracks every target (an app, the host, a certificate…) separately:

```text
inactive ──condition true──▶ pending ──true for the "for" duration──▶ firing ──condition false──▶ resolved
```

A condition that clears before the **for** duration ends never fires, so short spikes don't page you. With **for** set to immediately, the first true sample fires. Missing data holds the current state.

Rules are checked on every 30-second metrics tick, and immediately when a deployment, snapshot, certificate, cron job, container event or uptime check changes.

- **Acknowledge** a firing alert to stop its repeat notifications.
- **Snooze** a whole rule, or a single alert, for 1 minute to 30 days. You can also snooze a single alert until it resolves.
- **Test** a rule to send a sample notification to its channels.

Resolved alerts stay in the history for 90 days by default.

## Channels

Alerts are delivered through the channels you set up on the **Integrations** page:

| Channel | How it sends |
| --- | --- |
| Slack | Incoming webhook (also works with Mattermost) |
| Discord | Webhook |
| Telegram | Bot API |
| Email | SMTP |
| ntfy | Publish to your ntfy server, with an optional token |
| Pushover | Pushover API |
| Gotify | Your Gotify server |
| Webhook | `POST` of a JSON payload, signed with HMAC-SHA256 in `X-Tond-Signature-256` when you set a signing secret |

Channel secrets such as bot tokens and webhook URLs are stored encrypted. A failed delivery is retried after 30 seconds, 2 minutes, 10 minutes, 30 minutes and 2 hours. After six attempts it is marked failed. Failures show on the Alerts page and as the channel's last error, so nothing fails silently.

### Quiet hours

Quiet hours hold notifications during a daily window, for example 22:00 to 07:00 in `Europe/Berlin`. The window may span midnight. Set them on the **Alerts** page (**Quiet hours** tab). Each channel follows the global window, has none, or has its own.

Critical alerts are delivered anyway, unless you turn that off. Held notifications are sent when the window ends. If the alert resolved in the meantime, neither message is sent.

## Permissions

| Action | Scope |
| --- | --- |
| See metrics, uptime and alert rules | `metrics:read` |
| Acknowledge and snooze alerts | `apps:control` |
| Create, edit and delete rules, channels, quiet hours, heartbeat | `settings:write` |

For Tond's own logs, its Prometheus endpoint and self-monitoring, see [Observability](/operations/observability). For retention settings, see [Retention](/operations/retention).

## Related

- [Alerts and heartbeat](/guides/alerts)
- [Apps](/concepts/apps)
- [Domains and certificates](/concepts/domains)
