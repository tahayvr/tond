# Tond

Tond runs your apps and databases on one cheap VPS. You push code (or point it at an image), and Tond builds it, starts it, gives it HTTPS on a domain, watches it, backs it up and tells you when something breaks. It is a small, self-hosted platform: a single server you own, a dashboard, a CLI and an HTTP API.

```sh
curl -fsSL https://tond.io/install | sudo bash
```

The [Quickstart](/quickstart) takes you from a fresh server to a deployed app with a custom domain in about five minutes.

## What you get

- **Deploys from Git or anywhere else.** Connect GitHub, GitLab or Gitea and deploy on push, or deploy a Dockerfile, a compose file, a prebuilt image or a tarball from your laptop or CI. When a repository has no Dockerfile, Tond works out how to build it.
- **Zero-downtime releases with rollback.** Each deploy builds a new image, starts it next to the old one, waits for it to become healthy and only then moves traffic. Rolling back is one click.
- **HTTPS by default.** Every app and the dashboard get certificates automatically. Until you have a domain, `<app>.<server-ip>.nip.io` (the IP with dashes) works right after install.
- **Preview deployments** for pull requests, on a separate domain so untrusted code never shares cookies with your dashboard.
- **Managed databases**: PostgreSQL, MySQL, MariaDB, Redis, KeyDB and MongoDB in containers next to your apps, linked to them over private networks.
- **Snapshots and backups** of apps, databases and Tond itself, deduplicated on disk and optionally encrypted to S3-compatible storage.
- **Metrics, uptime checks and alerts** to Slack, Discord, Telegram, email, ntfy, Pushover, Gotify or a webhook, plus a heartbeat so you hear about it when the whole server is down.
- **Users, roles and scoped API tokens**, and a hash-chained audit log of every change.
- **One-click updates** of Tond itself, with a snapshot first and an automatic rollback if the new version doesn't come up.

## Principles

**One cheap VPS.** Tond is built for a single small server, not a cluster. Everything, including its database (SQLite) and its reverse proxy, runs on that one machine, and there is nothing else to operate. See the [support matrix](/support-matrix) for the systems it runs on.

**Secure by default.** The dashboard is only served over HTTPS. App containers run with dropped capabilities and on their own networks, their ports are bound to localhost, and the proxy runs as an unprivileged user. Secrets are encrypted at rest, sessions and tokens are hashed, releases are signed and verified by the installer, and nothing on the network is trusted because it comes from `127.0.0.1`. Read the [threat model](/security/threat-model) for what that covers.

**Frugal.** Tond's own processes use a few dozen megabytes of memory when idle and wake up only when there is something to do: background work sleeps until its next deadline or until the data it cares about changes. That leaves the server's CPU, memory and disk for your apps. [Resource footprint](/operations/footprint) has the numbers.

## How it fits together

<figure class="diagram">
<svg viewBox="0 0 760 330" role="img" aria-labelledby="flow-title flow-desc" xmlns="http://www.w3.org/2000/svg">
<title id="flow-title">How requests reach your apps</title>
<desc id="flow-desc">Browsers and clients on the internet connect to tond-proxy on ports 80 and 443. The proxy terminates TLS and routes by host name: app domains go to the app containers on local ports, the dashboard domain goes to tond-server on 127.0.0.1:3001. tond-server drives Docker, keeps its state in SQLite, and tells the proxy about route changes over a control socket. The tond CLI on the server talks to tond-server over a root-only Unix socket.</desc>
<defs><marker id="flow-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="var(--muted-foreground)"/></marker></defs>
<g font-size="13" fill="var(--foreground)">
<rect x="10" y="130" width="120" height="64" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="70" y="158" text-anchor="middle" font-weight="600">Internet</text>
<text x="70" y="178" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">browsers, CI, API</text>
<rect x="190" y="110" width="160" height="104" rx="6" fill="var(--card)" stroke="var(--foreground)" stroke-width="1.5"/>
<text x="270" y="140" text-anchor="middle" font-weight="600">tond-proxy</text>
<text x="270" y="162" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">:80 and :443</text>
<text x="270" y="180" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">TLS, certificates</text>
<text x="270" y="198" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">routes by host name</text>
<rect x="430" y="20" width="150" height="56" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="505" y="44" text-anchor="middle" font-weight="600">app containers</text>
<text x="505" y="63" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">127.0.0.1:&lt;port&gt;</text>
<rect x="430" y="130" width="150" height="64" rx="6" fill="var(--card)" stroke="var(--foreground)" stroke-width="1.5"/>
<text x="505" y="156" text-anchor="middle" font-weight="600">tond-server</text>
<text x="505" y="176" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">API + dashboard :3001</text>
<rect x="640" y="100" width="110" height="44" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="695" y="127" text-anchor="middle">Docker</text>
<rect x="640" y="180" width="110" height="44" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="695" y="207" text-anchor="middle">SQLite</text>
<rect x="430" y="256" width="150" height="56" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="505" y="280" text-anchor="middle" font-weight="600">tond CLI</text>
<text x="505" y="299" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">root, on the server</text>
<g stroke="var(--muted-foreground)" stroke-width="1.4" fill="none" marker-end="url(#flow-arrow)">
<path d="M130 162 H186"/>
<path d="M350 135 C390 135 390 48 426 48"/>
<path d="M350 162 H426"/>
<path d="M580 150 C610 150 610 122 636 122"/>
<path d="M580 176 C610 176 610 202 636 202"/>
<path d="M505 252 V198"/>
<path d="M430 186 C400 200 380 205 354 200" stroke-dasharray="4 3"/>
</g>
<g font-size="11" fill="var(--muted-foreground)">
<text x="395" y="84" text-anchor="middle">app domains</text>
<text x="390" y="154" text-anchor="middle">dashboard</text>
<text x="392" y="226" text-anchor="middle">route changes</text>
<text x="555" y="236" text-anchor="middle">Unix socket</text>
</g>
</g>
</svg>
<figcaption>tond-proxy is the only thing listening on the internet. Everything else is bound to localhost or a Unix socket.</figcaption>
</figure>

- **tond-proxy** terminates TLS, obtains and renews certificates, and routes each request by host name. It runs as an unprivileged user and reads Tond's database read-only.
- **tond-server** is the API, the dashboard and the orchestrator: it drives Docker, runs builds, health checks, backups, metrics, alerts and updates, and keeps everything in one SQLite file.
- **The dashboard** is a static web app served by tond-server. Everything it does goes through the [HTTP API](/reference/api/), so anything you can click you can also script.
- **The `tond` CLI** runs on the server as root over a root-only socket, or from anywhere with an [API token](/concepts/access). See the [CLI reference](/reference/cli).

## Where to go next

- New here: [Quickstart](/quickstart).
- Learn the model: start with [Apps](/concepts/apps) and [Deploys and rollback](/concepts/deploys).
- Automate: [CI with a deployer token](/guides/ci) and the [HTTP API](/reference/api/).
- For agents: [llms.txt](/llms.txt) lists every page as Markdown; [llms-full.txt](/llms-full.txt) is the whole site in one file.
