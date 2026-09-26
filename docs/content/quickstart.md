# Quickstart

From a fresh server to a deployed app on its own domain, in about five minutes.

## What you need

- A VPS running **Debian 12, Debian 13, Ubuntu 24.04 LTS or Ubuntu 26.04 LTS**, x86_64 or ARM64 ([support matrix](/support-matrix)). A fresh one is best.
- At least **2 GB of memory** (4 GB is comfortable) and **20 GB of disk**.
- A public IPv4 address, and ports **80 and 443** open in your provider's firewall. SSH stays open too.
- Root access over SSH.

No domain yet? That's fine: Tond uses `<server-ip>.sslip.io` for the dashboard and `<app>.<ip-with-dashes>.nip.io` for apps (for example `hello.203-0-113-10.nip.io`) until you bring your own.

## 1. Install

SSH into the server and run the installer:

```sh
curl -fsSL https://tond.io/install | sudo bash
```

It checks the system, installs Docker from Docker's signed apt repository, downloads the latest Tond release, verifies its checksum and signature, and starts `tond-server` and `tond-proxy`. It asks two questions:

- **Dashboard domain.** Press Enter to use `<server-ip>.sslip.io`, which already resolves to your server, or type your own domain (point an `A` record at the server first, or soon after).
- **Preview domain** (optional). Previews of pull requests need a separate domain you own; skip it for now.

For a scripted install, answer nothing and accept the defaults with `-y`, or pass the answers as flags (see [Configuration](/reference/configuration#installsh-flags)):

```sh
curl -fsSL https://tond.io/install | sudo bash -s -- -y --dashboard-domain dash.example.com
```

When it finishes, the installer prints a summary like this:

```text
╔════════════════════════════════════╗
║     Installation Complete!       ║
╚════════════════════════════════════╝

Server IP:  203.0.113.10
Dashboard:  https://203-0-113-10.sslip.io

Create your admin account
  Only someone with this one-time link can create the admin account, so keep it
  private until you have used it:

  https://203-0-113-10.sslip.io/setup?token=…

DNS:        Nothing to do: 203-0-113-10.sslip.io already resolves to this server.
            To use your own domain: sudo tond settings set-dashboard-domain <domain>

Back up your encryption key
  /opt/tond/data/tond.env holds TOND_SECRET_KEY. Every stored secret (env vars, database
  passwords, tokens) and every snapshot copy in remote storage is encrypted with it.
  …
Apps:       Served at <app>.203-0-113-10.nip.io (already resolves to this server).
```

> [!IMPORTANT]
> Copy `/opt/tond/data/tond.env` somewhere safe, off the server, now. It holds `TOND_SECRET_KEY`: without it, stored secrets and remote backups can't be decrypted. See [Environment and secrets](/concepts/secrets).

## 2. Create your admin account

Open the setup link from the summary. Pick your email and a password (at least 12 characters), and you're in. The link works once; if you lose it before using it, print it again with `sudo tond setup-link`.

The home page shows a short checklist of what's left: HTTPS for the dashboard (automatic once DNS points at the server), two-factor authentication, a connection to GitHub or a registry, your first app and a notification channel.

![The Tond home page with the "Finish setting up" checklist](/img/quickstart/home.light.webp)

## 3. Deploy your first app

Click **New app**. Give it a name (lowercase letters, numbers and hyphens), then choose where the code comes from:

- **Git**: paste a public repository URL, or connect GitHub for private ones ([Deploy from Git](/guides/deploy-from-git)).
- **Docker**: an image from a registry, such as `ghcr.io/you/app:latest`.
- **Templates**: a ready-made app such as Plausible ([Templates](/guides/templates)).

![The New app wizard: name, project, and the source step](/img/quickstart/new-app.light.webp)

The next steps set how it's built (a Dockerfile, or Tond's automatic detection), the port the app listens on, resources and environment variables. Every step has sensible defaults, so you can go straight to **Review** and create the app. The first deploy starts right away; its log streams live on the app's page.

Prefer the terminal? On the server, as root:

```sh
tond apps create hello --source https://github.com/you/hello --port 3000
tond apps deploy hello
```

`tond apps deploy` streams the build and exits with `0` once the new version is live (or non-zero if the build or health check fails).

When the deploy is done the app is running at `https://hello.203-0-113-10.nip.io` (your IP, with dashes), and its page shows its status, recent events, uptime and metrics.

![An app's overview: status, recent events and uptime](/img/quickstart/app.light.webp)

## 4. Give it your own domain

1. At your DNS provider, create an `A` record for the domain (say `hello.example.com`) pointing at the server's IP address.
2. In the dashboard, open the app → **Settings** → **Domain**, click **Add domain**, enter `hello.example.com` and click **Add domain** again. If the app already has its automatic address, the new domain is added as an alias; click **Make primary** on it to make it the app's main address. Or run `tond apps domain hello hello.example.com`, which adds it and makes it the primary in one step.
3. Tond checks the DNS record and tond-proxy gets a Let's Encrypt certificate as soon as the name resolves to the server. It usually takes under a minute. Select the domain in the list to follow its DNS and HTTPS status, or open the **Certificates** page.

![An app's Domain settings: its domains, with DNS and HTTPS status](/img/quickstart/domain.light.webp)

An app can have several domains. To send `www.hello.example.com` to the same place, add it as a redirect: `tond apps domains add hello www.hello.example.com --redirect-to hello.example.com`.

To serve every app under your own domain automatically (`<app>.apps.example.com`), set an apps domain with a wildcard DNS record. See [Domains and certificates](/concepts/domains).

## Next steps

- Turn on two-factor authentication under **Profile**, and add a notification channel so alerts reach you ([Alerts and heartbeat](/guides/alerts)).
- Add a database and link it to your app ([Databases](/concepts/databases)).
- Schedule snapshots and copy them to S3 ([Backups to S3](/guides/backups-s3)).
- Deploy from CI with a scoped token ([CI with a deployer token](/guides/ci)).
- Keep Tond current: **System → Updates** ([Upgrading Tond](/guides/upgrading)).
- Before you rely on it, go through the [security hardening checklist](/guides/hardening).
