# Threat model

Tond runs your apps and holds the keys to your server, so it is designed on the assumption that the internet is hostile and that the code it runs may be too. This page summarises what Tond defends against, where it draws trust boundaries, and what is out of scope.

## What Tond protects

- **The host**: root on the server, and through it every app and database.
- **The dashboard and API**: the admin's session, API tokens, and every action they allow.
- **Stored secrets**: environment variables, database passwords, Git and registry credentials, storage keys, 2FA secrets.
- **Your data**: volumes, managed databases, snapshots.
- **The record**: the audit log of who changed what.

## Trust boundaries

Tond trusts exactly two things without further checks: root on the host, and requests on the root-only Unix socket.

- **The root-only Unix socket** (`/opt/tond/data/tond.sock`, mode `0600`) is the only way to reach the API without credentials. The CLI on the server uses it. Its file permissions are the credential.
- **Loopback is not trusted.** `tond-proxy` forwards internet traffic to `tond-server` over `127.0.0.1`, so a request from loopback can come from anyone on the internet. Every TCP request must present a session cookie or an API token, whatever its source address.
- **Headers are not trusted.** No header grants access. `X-Forwarded-For` is believed only from the proxy on loopback, and the proxy overwrites whatever the client sent, so rate limits and token IP restrictions cannot be bypassed by spoofing it.

## Threats and defences

### Internet attackers

- `tond-proxy` is the only service that faces the internet. It parses untrusted TLS and HTTP, so it runs as the unprivileged `tond-proxy` user with only the capability to bind ports 80 and 443, a strict systemd sandbox, and read-only access to the database. It cannot reach Docker. See [Isolation](/security/isolation).
- `tond-server` listens on `127.0.0.1:3001` only.
- App and database ports are published on `127.0.0.1`, because Docker's port rules bypass the firewall. Only a managed database whose public access you enabled binds to all interfaces. Its [allowlist](/concepts/databases#allowed-addresses) is enforced in the kernel, and while a [major-version upgrade](/concepts/databases#major-version-upgrades) copies its data the port is closed to every address, so nothing writes to it that the copy would miss. The port reopens when the upgrade finishes or fails, and after a restart in the middle of one.
- The installer enables UFW with only SSH, 80 and 443 open.

### Brute force

- Every API request over TCP is rate limited per client (IPv6 grouped by `/64`).
- Failed sign-ins and 2FA attempts back off per client and per account, doubling up to 15 minutes. The per-account limit is higher than the per-client limit, so one attacker cannot lock the owner out quickly, and the CLI on the socket is never affected.
- Passwords need at least 12 characters, and the 10,000 most common passwords (plus Tond's own list) are refused, case-insensitively and with surrounding digits and symbols stripped, so `Password123!` counts as `password`. 2FA with TOTP is available for every account.

### Session theft

- Session cookies are named `__Host-tond_session`, marked `Secure`, `HttpOnly` and `SameSite=Strict`, with no `Domain`, so a sibling subdomain can neither read, set nor shadow them.
- The cookie holds a random 256-bit token. The database stores only its SHA-256 hash, so a leaked database or backup cannot be replayed as a login.
- Sessions end after an idle timeout (12 hours by default) and a maximum age (30 days by default). Disabling a user ends their sessions.
- The dashboard sends security headers: a Content-Security-Policy without `unsafe-inline` scripts, `frame-ancestors 'none'`, HSTS behind the TLS proxy, `nosniff` and a same-origin referrer policy.

### Cross-site request forgery

- Every state-changing request and every WebSocket upgrade with an `Origin` header must come from the dashboard's own origin.
- Cookie-authenticated state-changing requests must also send an `X-Tond-CSRF` header. Browsers only allow custom headers on cross-origin requests after a CORS preflight, and Tond answers no preflight, so only the dashboard can send it.
- WebSocket clients without an `Origin` have their cookies removed and must authenticate explicitly.

See [Error codes](/reference/error-codes) for what these rejections look like.

### Stolen or over-powered API tokens

- Tokens are scoped: each carries only the permissions it needs, capped by its owner's current role.
- Tokens can be limited to apps or projects, and to IP ranges. They expire (90 days by default; agent tokens must expire).
- Revocation takes effect immediately. Every token call is audited as that token.
- Tokens cannot open terminal sessions, because those need step-up re-authentication.

### Malicious or compromised app code

Apps, cron jobs and compose files may run code you did not write. Tond limits what that code can reach:

- Containers run with `no-new-privileges`, a trimmed capability set, a process limit and a default memory limit.
- Each workload has its own Docker network. Nothing joins Docker's default bridge. Databases are reachable only by the apps they are linked to.
- Users never choose host paths for volumes. Tond maps named volumes into its own directory and refuses symlink escapes.
- Compose files are checked before they are stored or deployed. Host-level options (`privileged`, the Docker socket, host namespaces, extra capabilities, host paths) are refused, and published ports are rewritten to `127.0.0.1`. `docker compose` runs with a cleared environment, so `${TOND_SECRET_KEY}` cannot leak into a container.
- Secret values are scrubbed from deploy and container logs.
- An app's outbound connections can be limited to an allowlist, or cut off, in the server firewall. Previews have no internet by default.

See [Isolation](/security/isolation) for the details.

### Pull-request previews

Previews run code from pull requests, possibly from strangers. They are served only on a separate registrable domain (`<id>-<app>.<preview_domain>`), checked against the Public Suffix List, so a preview page is cross-site to the dashboard and cannot use its cookies. `tond-proxy` never serves the dashboard or API on the preview domain. Without a preview domain, previews are refused. Previews never join the project's network, so they cannot reach production databases, and by default they can't connect to the internet either (DNS aside), so pull-request code can't send out what it reads or abuse the server's address. See [Previews](/concepts/previews) and [Outbound connections](/security/isolation#outbound-connections).

### Unclaimed admin accounts in one-click apps

Many self-hosted apps give the admin account to whoever opens them first (an install page, or "the first account becomes the admin"), or ship a fixed default login. Once such an app is on a public domain, a scanner can reach it before you do. Templates close this where the app allows it:

- **Pre-seeded**: Gitea, Mattermost, Actual Budget, Audiobookshelf, Umami and Mealie. Tond creates the admin (or replaces the default password) with a generated password while the app is running but before `tond-proxy` routes its domain, so nobody can reach it first. The password is shown once after the deploy and kept, encrypted, in the app's environment.
- **Warned**: Ghost, Uptime Kuma, Memos, Wiki.js, Docmost and Twenty have no documented way to create the admin from outside. Their app page warns "Open the app now and create the admin account; anyone who reaches it first can" until you dismiss it.

If pre-seeding fails, the deploy still finishes and the app page shows the same warning with the reason. See [Templates](/guides/templates#admin-accounts).

### The host terminal

The browser shell on the host is off by default and can only be switched on from the server itself (`sudo tond settings host-terminal on`), never from the dashboard or with a token. When on, it is admin-only, every session requires the password (and 2FA code) again, and it runs as an unprivileged user that is not in the `docker` or `sudo` groups. See [Host terminal](/guides/host-terminal).

### Secrets at rest

Stored secrets are encrypted with AES-256-GCM under a key derived from `TOND_SECRET_KEY`, which lives in a root-only file outside the database. Each value is bound to its column, so a ciphertext copied elsewhere does not decrypt. A copy of `tond.db` alone does not reveal the secrets. See [Environment and secrets](/concepts/secrets).

### Tampering with the record

The audit log is hash-chained and verified with `tond audit verify`. SQLite triggers refuse updates and deletes outside retention. Shipping the log to a syslog server or webhook lets you detect truncation or a rewritten chain. See [Audit log](/concepts/audit-log).

### Supply chain

- Release tarballs are signed with minisign. The installer, `update.sh`, the web installers and `tond update` pin the release, verify its SHA-256 checksum, and verify the signature with an embedded public key before running anything from it. A signature that does not verify is always fatal; a missing signature is refused unless you set `TOND_ALLOW_UNSIGNED=1`.
- Each tarball carries a SLSA build provenance attestation from GitHub Actions, which you can check with `gh attestation verify`.
- The installer installs Docker from Docker's apt repository, pinned to Docker's signing key fingerprint, rather than piping a convenience script to a shell.
- The Rust dependencies go through `cargo deny`, and the dashboard's through `pnpm audit`.

To verify a release by hand:

```sh
minisign -Vm tond-linux-x86_64.tar.gz -p minisign.pub
gh attestation verify tond-linux-x86_64.tar.gz --repo tahayvr/tond
```

## Out of scope

Tond does not defend against:

- **A compromised root account on the host.** Root can read the key file, the database and every container. Protect SSH (keys only, no password login) and keep the OS patched. See [Security hardening checklist](/guides/hardening).
- **Docker being root-equivalent.** `tond-server` runs as root because it drives Docker, and anyone who controls the Docker daemon controls the host. Tond does not use user namespaces or rootless Docker.
- **Kernel or container runtime escapes.** Containers share the host kernel. Hardening narrows the attack surface, but a kernel exploit from inside a container reaches the host.
- **Malicious admins.** An admin can deploy any image and read every secret. The audit log records what they do; it does not stop them.
- **Your images and dependencies.** Tond does not scan the images or packages your apps use.
- **Network-level denial of service.** Rate limits protect the API, but a flood of traffic to the server is a job for your provider or a CDN.
- **Your cloud provider and DNS.** Whoever controls your DNS can point your domains elsewhere and obtain certificates for them.
- **Lost keys.** If you lose `TOND_SECRET_KEY`, stored secrets and remote snapshot copies cannot be recovered. Back up `/opt/tond/data/tond.env`.

Found a gap in any of this? See [Reporting vulnerabilities](/security/reporting).
