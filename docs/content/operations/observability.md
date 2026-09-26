# Observability

This page is about watching Tond itself: its logs, its health, and its metrics. For your apps' metrics, uptime checks and alert rules, see [Metrics, uptime and alerts](/concepts/observability).

## Logs

`tond-server` and `tond-proxy` log to the systemd journal. Read them with `journalctl`:

```sh
# Follow the server's log
journalctl -u tond-server -f

# The last 100 lines of the proxy's log
journalctl -u tond-proxy -n 100 --no-pager

# Everything from both since the last boot
journalctl -u tond-server -u tond-proxy -b
```

At the default level Tond logs little when idle (the resource test allows at most 10 lines per hour per service), so what you see is usually worth reading.

### Log level

`RUST_LOG` sets the level. The units set `info`. For more detail while you debug:

```sh
sudo systemctl edit tond-server
```

```ini
[Service]
Environment=RUST_LOG=debug
```

```sh
sudo systemctl restart tond-server
```

`RUST_LOG` also accepts per-module directives, for example `RUST_LOG=info,tond_server::deploy=debug`. Set it back to `info` when you are done.

### JSON logs

For a log shipper (Vector, Promtail, Fluent Bit reading the journal), switch either service to one JSON object per line with `TOND_LOG_FORMAT=json`:

```ini
[Service]
Environment=TOND_LOG_FORMAT=json
```

Each line looks like this. Event fields sit next to `message`; the current span's fields sit under `span`:

```json
{"timestamp":"…","level":"INFO","target":"tond_server::deploy","message":"…","span":{"name":"…"}}
```

Any other value, or none, gives the readable text format. See [Configuration](/reference/configuration) for how to set variables.

The journal is capped at 500 MB by the installer unless you set your own limit.

## The System page

The dashboard's **System** page shows the server and Tond's own health:

- **Overview**: host CPU, memory, disk and network; Docker's state; the heartbeat; and a Tond health panel.
- **History**: host history over time.
- **Disk**: what fills the disk (images, build cache, containers, volumes, Tond's data), a disk-full forecast, and **Free up space**.
- **Uptime**: the uptime checks, including the built-in check of the dashboard.

The Tond health panel shows both processes (memory, CPU, open files, threads), the database (size, write-ahead log, query counts, slow queries), HTTP request latency, the build queue, the proxy's connection and route version, and every background loop.

### Background loops

Tond's background work runs in named loops: `metrics`, `health`, `uptime`, `alerts`, `cron`, `backup-scheduler`, `janitor`, `preview-cleanup`, `audit-shipping`, `proxy-link` and `route-nudge`. For each loop the panel shows when it last ran, how long it took, its wakeups, its lag and its last error. A loop that stops running or keeps failing is the first thing to look at when something in Tond stops happening, such as cron jobs not firing or metrics not updating.

### /api/v1/system/self

The same numbers are available as JSON:

```sh
sudo curl --unix-socket /opt/tond/data/tond.sock http://tond/api/v1/system/self
```

Over TCP it needs a token or session with the `metrics:read` permission. See [HTTP API](/reference/api/).

## Prometheus metrics

`/metrics` serves the same numbers in the Prometheus text format. It needs the `metrics:read` permission.

On the server itself, the root-only socket needs no token:

```sh
sudo curl --unix-socket /opt/tond/data/tond.sock http://tond/metrics
```

From a Prometheus server, scrape the dashboard domain over HTTPS with an API token that has only `metrics:read`:

```sh
sudo tond token create --name prometheus --scope metrics:read --expires never
```

Save the token to a file on the Prometheus host, then:

```yaml
scrape_configs:
  - job_name: tond
    scheme: https
    static_configs: [{ targets: ['dash.example.com'] }]
    authorization:
      credentials_file: /etc/prometheus/tond-token
```

Settings → Observability shows this snippet with your dashboard domain filled in.

The metrics include:

| Metric | What it measures |
| --- | --- |
| `tond_uptime_seconds` | Time since `tond-server` started |
| `tond_process_resident_memory_bytes`, `tond_process_cpu_seconds_total`, `tond_process_open_fds`, `tond_process_threads`, `tond_process_context_switches_total` | The `tond-server` process |
| `tond_proxy_connected`, `tond_proxy_resident_memory_bytes`, `tond_proxy_cpu_seconds_total`, `tond_proxy_open_fds`, `tond_proxy_routes`, `tond_proxy_route_version`, `tond_proxy_route_reloads_total`, `tond_proxy_loop_wakeups_total` | The `tond-proxy` process, as reported over its control socket |
| `tond_loop_runs_total`, `tond_loop_errors_total`, `tond_loop_wakeups_total`, `tond_loop_last_duration_seconds`, `tond_loop_max_lag_seconds` | Background loops |
| `tond_db_size_bytes`, `tond_db_wal_bytes`, `tond_db_queries_total`, `tond_db_slow_queries_total` | The SQLite database |
| `tond_http_request_duration_seconds` | API request latency per route (histogram) |
| `tond_build_slots`, `tond_deployments_active`, `tond_deployments_queued` | The build queue |
| `tond_tokio_alive_tasks`, `tond_tokio_worker_parks_total` | The async runtime |

Useful alerts: `tond_proxy_connected == 0`, a rising `tond_loop_errors_total`, and `tond_deployments_queued` staying above zero for a long time.

API requests over TCP are rate limited per client (a burst of 60, then 10 per second). A normal scrape interval stays far below that.

## Audit log shipping

The audit log records every change with the authenticated actor, in a hash chain that detects edits and deletions. Someone with root on the server could still cut off the newest entries or rewrite the whole chain. Shipping each entry to a system the server cannot rewrite closes that gap.

Configure it under **Settings → Audit log shipping**. It is off by default. Two destinations are supported:

- **Syslog**: RFC 5424 messages over TCP, normally over TLS on port 6514, one message per entry.
- **Webhook**: HTTPS POST of batches of entries, signed with an HMAC-SHA256 signature in `X-Tond-Signature` and a timestamp in `X-Tond-Timestamp`. Any 2xx response counts as delivered.

Delivery resumes where it left off after a restart. Failed deliveries are retried with backoff. Entries that retention removes before they are delivered are counted as dropped. See [Audit log](/concepts/audit-log) and [Retention](/operations/retention).

## Heartbeat

A server cannot report its own outage. The heartbeat is a dead-man's switch: Tond pings an outside monitor (healthchecks.io, an Uptime Kuma push monitor, a Better Stack heartbeat and similar) on a schedule, every 5 minutes by default (1 minute to 24 hours). When the pings stop, that service alerts you.

Tond pings only while it is healthy, which means all of:

- Docker answers,
- `tond-proxy` is connected over its control socket,
- the metrics loop ran in the last two minutes,
- the database answers.

Otherwise the ping is skipped, the reason is recorded, and the monitor alerts after its grace period. The ping URL is treated as a secret: it is stored encrypted, only its host is shown after saving, and it is never logged.

Configure it under **Settings → Heartbeat**. The System page shows the last outcome. See [Alerts and heartbeat](/guides/alerts).
