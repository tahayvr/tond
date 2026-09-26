# Security hardening checklist

Tond ships with safe defaults, but a server is only as safe as its weakest door. Work through this checklist after installing Tond, and again whenever you add people or automation. Each item says why it matters and where to change it. What Tond defends against, and what it doesn't, is described in the [Threat model](/security/threat-model).

## The server

**Use SSH keys only.** Add your public key, then set `PasswordAuthentication no` and `PermitRootLogin prohibit-password` (or `no`, if you log in as another user and use `sudo`) in `/etc/ssh/sshd_config`, and reload `sshd`. Test a new session before closing the old one.

**Keep the host firewall on.** If UFW is installed, the installer sets it up: when UFW was inactive it denies incoming traffic by default, allows outgoing traffic, and opens only your SSH port(s), `80/tcp` and `443/tcp`; when UFW was already active it adds those three rules. Check with:

```sh
sudo ufw status verbose
```

Don't open other ports for Tond. It needs none.

**Close everything else at the provider too.** If your provider offers a cloud firewall, allow only 80 and 443 from anywhere, and SSH from the addresses you work from. A cloud firewall also covers what UFW can't see.

**Know that Docker bypasses UFW.** A port Docker publishes on `0.0.0.0` is reachable from the internet whatever UFW says. Tond avoids this: managed database ports and compose ports are published on `127.0.0.1` only. Keep it that way:

- Leave **public access** off on managed databases (it is off by default and admin-only). Reach a database from your machine through an SSH tunnel instead: `ssh -L 5432:127.0.0.1:<host-port> root@your-server`.
- Don't run containers with published ports outside Tond on the same server.

**Install security updates automatically.** On Ubuntu or Debian:

```sh
sudo apt install unattended-upgrades
sudo dpkg-reconfigure --priority=low unattended-upgrades
```

Reboot when a kernel update needs it (`/var/run/reboot-required` exists), at a quiet time.

## Tond itself

**Keep Tond updated.** Releases carry security fixes. See [Upgrading Tond](/guides/upgrading).

**Back up `TOND_SECRET_KEY` offline.** `/opt/tond/data/tond.env` holds the key that encrypts every stored secret and every remote snapshot copy. Keep a copy off the server, apart from your backups, for example in a password manager. Without it, those secrets and backups can't be recovered.

**Ship encrypted backups off the server.** Configure an S3-compatible storage provider and scheduled snapshots, and add a **Backup failed** alert. See [Backups to S3](/guides/backups-s3).

**Turn the host terminal off** if you don't use it. It is off by default; check with `tond settings`, and switch it off with:

```sh
sudo tond settings host-terminal off
```

Even when on, it runs as an unprivileged user, but less surface is better. See [Host terminal](/guides/host-terminal).

**Use separate domains.** Previews run pull-request code, and apps run your code. Serve them on registrable domains separate from the dashboard's, so they can never touch its cookies: set the **Apps Domain** and **Preview Domain** under **Settings** (or `tond settings set-apps-domain` and `tond settings set-preview-domain`). Tond refuses a preview domain on the dashboard's site and refuses previews without one. See [Domains and certificates](/concepts/domains) and [Previews](/concepts/previews).

## People

**Turn on two-factor authentication for every admin.** Each user enables it under **Profile → Two-factor**. Tond doesn't force it, so check your admins have done it. The host terminal asks for the 2FA code on every session once it is on.

**Give people the smallest role that works.** Tond has three roles: admin (everything), deployer (ships code: deploy, restart, roll back, previews, write environment variables without reading them) and viewer (read-only, no secret values). Keep the number of admins small. See [Users, roles and tokens](/concepts/access).

**Shorten sessions.** Under **Settings → Dashboard sessions**, set **Sign out after inactivity (hours)** and **Maximum session lifetime (days)** to what suits you. Each user can review and sign out other sessions under **Profile → Sessions**.

**Remove people when they leave.** Delete their account on the **Users** page and revoke the tokens they created.

## Automation

**Scope every API token.** Start from a preset (`readonly`, `deployer`, `agent-operator`) rather than picking admin scopes, and limit the token to the apps or projects it needs:

```sh
sudo tond token create --name ci-web --kind ci --preset deployer --app web --expires 30d
```

Presets never include admin scopes, and a token can never do more than the user who created it. See [CI with a deployer token](/guides/ci).

**Make tokens expire.** Tokens default to 90 days; agent tokens must expire within 30 days. Use `never` only for tokens you rotate by hand and watch closely.

**Pin tokens to addresses** when the caller has a fixed IP, with `--allow-ip` (or **Allowed IPs** in the dashboard).

**Review tokens regularly.** `tond token list --all` lists every user's tokens; revoke what you don't recognise with `tond token revoke <name-or-id>`.

**Treat webhook secrets as passwords.** Every webhook must be signed and verification can't be turned off. If a secret leaks, rotate it (**Settings → Git → Webhooks → Rotate secret**, or `tond apps git-webhook <app> --rotate-secret`).

**Use read-only credentials for registries and Git hosts.** Deploy keys are read-only; registry tokens and GitLab access tokens should be too, where the provider allows it.

**Review compose files.** Only admins can create compose apps, and Tond rejects host-level options (privileged mode, host namespaces, host binds, Docker socket). Still read a compose file before you deploy it. See [Build sources](/guides/sources).

## Watching

**Ship the audit log off the server.** Under **Settings → Audit log shipping**, send every entry to a **Syslog server** (with **Use TLS**) or an **HTTPS webhook** with a **Signing secret**. A copy elsewhere survives a compromised server. Check the local chain at any time:

```sh
sudo tond audit verify
```

It exits with status 1 if the hash chain was tampered with. See [Audit log](/concepts/audit-log).

**Set up alerts and a heartbeat.** At least **App down**, **Disk usage** and **Certificate expiry** rules, plus a heartbeat to an outside monitor so you learn when the whole server stops. See [Alerts and heartbeat](/guides/alerts).

## Checklist

- [ ] SSH keys only, password login off
- [ ] UFW active with only SSH, 80 and 443 open
- [ ] Cloud firewall: 80 and 443, SSH from your addresses
- [ ] No database with public access
- [ ] Unattended security updates on
- [ ] Tond on the latest release
- [ ] `tond.env` copied offline
- [ ] Remote encrypted snapshots on a schedule, with a **Backup failed** alert
- [ ] Host terminal off, unless needed
- [ ] Apps domain and preview domain separate from the dashboard's
- [ ] 2FA on for every admin
- [ ] Fewest possible admins; short dashboard sessions
- [ ] Every token scoped, limited to its apps, and expiring
- [ ] Audit log shipped off the server
- [ ] Alerts and a heartbeat configured
