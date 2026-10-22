# Alerts and heartbeat

This guide sets up notifications so you hear about problems before your users do: a channel to deliver them, alert rules that decide what is worth a message, uptime checks for your public URLs, and a heartbeat that tells an outside service when the whole server stops. You need an admin account (creating channels and rules needs `settings:write`).

How rules are evaluated is described in [Metrics, uptime and alerts](/concepts/observability).

## 1. Add a notification channel

Open **Integrations**. Under **Notification Channels**, pick a service, click **Add Channel**, give it a **Display Name** and fill in its fields:

| Channel | What you need |
| --- | --- |
| Telegram | **Bot Token**, **Chat ID**, optionally a **Thread / Topic ID** |
| Discord | **Discord Webhook URL** |
| Slack / Mattermost | **Incoming Webhook URL** |
| Email (SMTP) | **From Name**, **From Address**, **SMTP Host**, **SMTP Port**, **SMTP Username**, **SMTP Password**, **Encryption** (TLS, STARTTLS or none), **Recipient Addresses** |
| ntfy | **Server URL** (default `https://ntfy.sh`), **Topic**, optionally an **Access token** |
| Pushover | **User key**, **Application API token** |
| Gotify | **Server URL**, **Application token** |
| Webhook | **URL**, optionally a **Signing secret** |

Click **Save Channel**, then **Test** to send a test message. Fix the channel until the test arrives.

Tokens, webhook URLs, passwords and ntfy topics are stored encrypted and never shown again; when you edit a channel, leave a secret field alone to keep the saved value. On a public ntfy server, the topic name works like a password, so pick a long random one.

The **Webhook** channel POSTs each alert as JSON. With a signing secret, Tond signs the body with HMAC-SHA256 and sends the signature in the `X-Tond-Signature-256: sha256=<hex>` header, so your receiver can verify it.

From the CLI you can list, test, enable, disable and delete channels (`tond integrations …`); creating them is done in the dashboard or the API.

## 2. Create alert rules

Open **Alerts** and click **New rule**.

1. **What to watch**: pick a kind (see the table below). Kinds whose data this server doesn't have yet are marked "Not available yet".
2. **Name**: for example "App down on production".
3. **Apps**: **Every app** or a single app, for app kinds.
4. The threshold and **Within the last**, where the kind has them.
5. **Fire after it lasts**: how long the condition must hold before anyone is notified. Short spikes that end sooner never notify.
6. **Remind while it's firing**: how often to repeat while nobody has acknowledged it (default every 4 hours, or never).
7. **Severity**: info, warning or critical.
8. **Notify**: at least one channel. A rule without a channel can't be saved.

Click **Create rule**. On the rule's page, **Send test** sends a test to every channel of the rule.

| Kind | Fires when | Default |
| --- | --- | --- |
| App down | The app isn't running (crashed or failed to start), or its public URL fails the uptime check. Stopped apps don't count. | after 2 min, critical |
| Crash loop | The app restarted after crashing this many times within the window | 3 in 10 min |
| Out of memory | A process in the app was killed for running out of memory | within 15 min |
| CPU usage | CPU above this percent of one core | 80 % for 5 min |
| Memory usage | Memory above this percent of the app's limit | 90 % for 5 min |
| 5xx error rate | Share of requests answered with a 5xx status | 5 % over 5 min |
| Slow responses (p95) | 95th percentile response time | 1000 ms over 5 min |
| Deploy failed | The latest deploy failed; resolves on a successful deploy | immediately |
| Disk usage | The disk holding Tond's data is this full | 90 % for 5 min, critical |
| Disk full forecast | At the last 24 hours' growth rate, the disk fills within this many hours | 48 h |
| Automatic disk cleanup | Tond pruned images and build cache because the disk crossed the cleanup threshold; the value is the disk use afterwards. Resolves an hour later | immediately |
| Certificate expiry | A TLS certificate expires within this many days | 14 days |
| Backup failed | The latest snapshot or its upload failed | immediately, critical |
| Cron job failed | A cron job's latest run failed after its retries | immediately |

A good starting set: **App down** and **Deploy failed** for every app, **Disk usage**, **Certificate expiry** and, once backups run, **Backup failed**.

## 3. Snooze, acknowledge and quiet hours

When an alert fires it appears on the **Alerts** page with actions:

- **Acknowledge** stops the reminders for this episode. It still resolves on its own.
- **Snooze** mutes notifications for this alert for 1 hour, 8 hours, 24 hours or until it resolves. Alerts still fire and show in the dashboard.
- On a rule's page, **Mute** silences the whole rule for a while.

These need `apps:control`, so deployers can handle alerts too.

**Quiet hours** hold notifications back during a daily window, such as 22:00 to 07:00, and send them when it ends. Set them under **Alerts → Settings**: turn on **Hold back notifications during quiet hours**, choose **From**, **Until** and **Time zone**, and keep **Critical alerts come through anyway** on unless you really want silence. Each channel can follow the global quiet hours, have none, or have its own (the channel's **Quiet hours** setting). Use that to keep, say, email quiet at night while a pager channel always rings.

Failed deliveries are retried with backoff. An alert's history shows every delivery and its error.

## 4. Uptime checks

Tond checks the public URL of every app that has a domain, and the dashboard itself, from the outside path: DNS, TCP, TLS and HTTP. A check is created automatically when an app gets a domain. A failed probe is retried once before it counts, so a single dropped packet doesn't open an incident. Stopped apps and background workers are paused.

Open an app's **Uptime** tab to see uptime over 24 hours to 90 days, response times and incidents. Under **Check settings** you can change the **Path**, the **Interval** (every minute to every hour), the **Timeout (seconds)** and the **Expected status**, or turn the check off. The dashboard's own check is under **System → Uptime**.

A failing uptime check makes an **App down** rule fire, so the rule from step 2 covers both crashes and unreachable URLs.

## 5. Heartbeat: learn when the whole server is down

A server can't report its own outage. The heartbeat is a dead-man's switch: Tond pings an outside monitor on a schedule, and when the pings stop, that monitor alerts you.

1. Create a check in an external service and copy its ping URL. For example:
   - **Healthchecks.io**: add a check, set its period to your interval and a grace time of a few minutes, copy the `https://hc-ping.com/…` URL.
   - **Uptime Kuma**: add a monitor of type Push with the same interval, copy the push URL.
   - **Better Stack**: create a heartbeat with the same period, copy its URL.

   Use a monitor that runs somewhere else; a monitor on this server stops with it. A grace period of about twice the interval avoids pages for one slow ping.
2. In Tond, open **Settings** and find **Heartbeat (dead-man's switch)**.
3. Paste the **Ping URL** (it must start with `https://`), choose the **Interval** (every minute to every hour, default 5 minutes) and the **Request** method (GET or POST), and keep **Send heartbeats** on.
4. Click **Save**, then **Send test ping** and confirm the monitor received it.

Tond only pings while it is healthy: Docker answers, tond-proxy is connected, the metrics collector ran in the last two minutes and the database answers. Otherwise the ping is skipped, the section shows **Holding pings** with the reason, and your monitor alerts after its grace period. This catches a wedged server as well as a dead one.

The ping URL is a secret. Tond stores it encrypted and only shows its host. Before you remove the heartbeat or take the server down on purpose, pause the check in your monitor, or it will alert.

## Next steps

- [Operations: observability](/operations/observability) covers logs and Prometheus metrics.
- [Retention](/operations/retention) lists how long alert history and uptime data are kept.
