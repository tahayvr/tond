# Troubleshooting

Most problems with a new Tond server come down to DNS, ports or firewalls. This page walks through the common ones, from certificates to a lost admin account, with the commands to diagnose them.

## First checks

Start here whatever the symptom:

```sh
systemctl status tond-server tond-proxy docker
journalctl -u tond-server -n 100 --no-pager
journalctl -u tond-proxy -n 100 --no-pager
curl -sf http://127.0.0.1:3001/health && echo ok
```

The System page in the dashboard shows the same health information, including every background loop and its last error (see [Observability](/operations/observability)).

## Certificates

`tond-proxy` gets certificates from Let's Encrypt with the ACME **HTTP-01** challenge. For each domain it serves (the dashboard domain, app domains, custom domains), the CA connects to `http://<domain>/.well-known/acme-challenge/…` on port 80, and the proxy answers. That only works when:

- the domain's DNS points at this server,
- port 80 is reachable from the internet (both the server's firewall and your cloud provider's firewall),
- nothing else on the server holds port 80.

Wildcard certificates are not used: each hostname gets its own certificate.

### Where to see the status

The **Certificates** page lists every domain with its certificate status, expiry and the last error. Settings shows the dashboard domain's status next to the field. Certificates are renewed when they are within 30 days of expiry.

### Failed issuance

When issuance fails, the proxy backs off: 1 minute after the first failure, doubling up to 6 hours. The error is recorded and shown on the Certificates page. Use **Retry now** there once you have fixed the cause. Saving the domain again also resets the backoff.

Before it orders a certificate, the proxy resolves the domain. If the domain resolves only to public addresses and none of them is this server, the order is skipped with a message such as:

```text
DNS for app.example.com points to 198.51.100.4, expected 203.0.113.7. Update the domain's A/AAAA record to this server.
```

Fix the DNS record and retry. Domains behind a CDN or proxy that resolve elsewhere but still forward HTTP-01 requests to the server are attempted anyway once the backoff reaches 6 hours.

> [!NOTE]
> `tond-proxy` listens on IPv4 only (`0.0.0.0:80` and `0.0.0.0:443`). If a domain has an AAAA record, Let's Encrypt may try IPv6 first and fail to connect. Remove the AAAA record, or point it at something that forwards to the server's IPv4 address.

### Rate limits

Let's Encrypt limits how many certificates you can get for the same names in a week, and how many failed validations you can have per hour. The exact limits are on the Let's Encrypt website. If the Certificates page shows a rate-limit error, wait for the window to pass. Tond's backoff avoids hammering the CA while you fix DNS.

For testing, point the proxy at the Let's Encrypt staging CA, which has much higher limits but issues certificates browsers do not trust:

```sh
sudo systemctl edit tond-proxy
```

```ini
[Service]
Environment=TOND_ACME_STAGING=true
```

Remove it again for production. See [Configuration](/reference/configuration).

## DNS

The installer picks working defaults when the server has a public IPv4 address:

| Domain | Default |
| --- | --- |
| Dashboard | `<ip-with-dashes>.sslip.io`, for example `203-0-113-7.sslip.io` |
| Apps | `<ip-with-dashes>.nip.io` (or `.sslip.io` when the dashboard is on nip.io) |
| Previews | none; previews stay off until you set a preview domain |

sslip.io and nip.io resolve any name that contains an IP address to that address, including subdomains, so they work without any DNS setup. They are fine for trying Tond; use your own domains for production.

With your own domains you need:

| Record | Points to | Needed for |
| --- | --- | --- |
| `A dash.example.com` | the server's IPv4 address | the dashboard |
| `A *.example-apps.com` | the server's IPv4 address | automatic app hostnames `<app>.example-apps.com` |
| `A *.example-previews.com` | the server's IPv4 address | previews `<id>-<app>.example-previews.com` |
| `A www.customer.com` | the server's IPv4 address | each custom domain |

The apps domain and the preview domain must each be a different site (registrable domain) from the dashboard, because apps and previews run code that should not share cookies with the dashboard. Tond refuses domains that break this rule and says why. See [Domains and certificates](/concepts/domains) and [Previews](/concepts/previews).

To check what a name resolves to from the outside:

```sh
dig +short A dash.example.com
dig +short AAAA dash.example.com
```

## Ports

Tond needs ports 80 and 443 for `tond-proxy`. The installer's preflight warns when either is already in use. Find out what holds them:

```sh
sudo ss -tlnp | grep -E ':(80|443) '
```

A web server installed by the distribution (nginx, Apache, Caddy) is the usual cause. Stop and disable it, then restart the proxy:

```sh
sudo systemctl disable --now nginx
sudo systemctl restart tond-proxy
```

`tond-server` listens on `127.0.0.1:3001` only. It is not meant to be reached from outside; the proxy forwards dashboard and API traffic to it.

## Firewall

The installer configures UFW when it is installed:

- If UFW is active, it allows your SSH port(s), 80/tcp and 443/tcp.
- If UFW is installed but inactive, it asks whether to enable it (with `--non-interactive` it enables it). It then sets incoming traffic to deny by default, outgoing to allow, and allows SSH, 80 and 443.

SSH ports are detected from your current SSH session, `sshd`'s configuration, its listening sockets and systemd's `ssh.socket`. Port 22 is used when none is found. Check the result:

```sh
sudo ufw status verbose
```

Your cloud provider's firewall (security groups, Hetzner Cloud firewall and similar) is separate from UFW and Tond cannot change it. It must allow inbound TCP 80 and 443 as well. The installer reminds you of this in its preflight summary.

> [!WARNING]
> Docker's port publishing bypasses UFW. Tond publishes app and database ports on `127.0.0.1` only, so they are not reachable from outside. The exception is a managed database whose public access you turned on: it binds to all interfaces, and UFW does not block it.

## Services not starting

```sh
systemctl status tond-server
journalctl -u tond-server -b --no-pager
```

Common causes:

- **Missing encryption key.** Release builds refuse to start without `TOND_SECRET_KEY`. The log says:

  ```text
  TOND_SECRET_KEY is not set. Generate one with `openssl rand -hex 32`, write `TOND_SECRET_KEY=<key>` to /opt/tond/data/tond.env (mode 0600) and restart tond-server.
  ```

  If you had a key before, restore `tond.env` from your backup instead of generating a new one. A new key cannot decrypt secrets stored with the old one.

- **Docker is not running.** `tond-server` requires `docker.service`. Check `systemctl status docker`.

- **Port conflict.** Another process holds 3001 (server) or 80/443 (proxy). See the Ports section above.

- **Hitting the memory cap.** The units set `MemoryMax` (1 GB for the server, 384 MB for the proxy). `systemctl status` shows `oom-kill` when that happens. See [Resource footprint](/operations/footprint).

- **Changed file permissions.** The proxy reads `tond.db` through the `tond-proxy` group and must own `certs/`. Running the installer again restores the expected permissions without touching your data.

`tond-proxy` starts after `tond-server`. If the proxy runs but the dashboard shows an error page, check that `tond-server` is up and that `TOND_SERVER_ADDR` matches the server's `TOND_BIND` and `TOND_PORT`.

## Docker problems

```sh
systemctl status docker
docker info
docker ps -a --filter label=managed-by=tond
```

- **Docker unreachable**: the System page shows Docker's state, and the heartbeat stops pinging while Docker does not answer. Restart it with `sudo systemctl restart docker`. Containers come back up because Tond starts them with the `unless-stopped` restart policy.
- **A build killed with no clear error**: builds share a memory cap of 60 % of RAM (`tondbuild.slice`). A build that needs more is killed. Reduce the build's memory use, or raise the cap with `sudo systemctl edit tondbuild.slice`.
- **An app restarting or killed**: the app's events show restarts, crashes and out-of-memory kills. Apps without an explicit memory limit are capped at 75 % of RAM.
- **Edited `/etc/docker/daemon.json` that is invalid JSON**: the installer leaves it alone and warns. Docker itself will not start until it is fixed.

## Disk full

A full disk breaks builds, database writes and log collection. Look at **System → Disk** first: it shows what fills the disk (Docker images, build cache, containers, volumes, Tond's data, everything else) and a forecast of when the disk fills up.

**Free up space** removes unused build cache and dangling images only. It shows exactly what it will remove before you confirm. Your apps' images (including the ones kept for rollback), containers, volumes and backups are never touched.

If that is not enough:

- Lower the snapshot retention or turn off the local copy when you ship snapshots to S3 (see [Snapshots and backups](/concepts/backups)).
- Lower the retention of deploy logs and history (see [Retention](/operations/retention)). Settings → Data retention shows which tables are largest.
- Check your apps' volumes under `/opt/tond/data/volumes`.
- Check the journal: `journalctl --disk-usage`.

## Lost setup link

On a fresh install, the installer prints a one-time link for creating the first admin account. If you lost it, print it again on the server:

```sh
sudo tond setup-link
```

The token behind the link is stored in a root-only file and removed once the admin account exists. After that the command reports that no setup is pending; sign in to the dashboard instead.

## Locked out

If you forgot your password, another admin can reset it, or you can create a one-time reset link on the server as root:

```sh
sudo tond users list
sudo tond users reset-link you@example.com
```

The link is valid for 24 hours and works once. The CLI on the server talks to `tond-server` over the root-only socket, so it needs no dashboard login.

If sign-in is refused with "Too many failed sign-in attempts", wait for the time shown. The backoff doubles with each failure up to 15 minutes, and is forgotten after 15 minutes without a new failure. The CLI is never affected. See [Users, roles and tokens](/concepts/access) and [Error codes](/reference/error-codes).
