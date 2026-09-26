# Users, roles and tokens

Tond decides what every caller may do with **scopes**: single permissions such as `apps:deploy` or `db:credentials`. Users get their scopes from their **role**. API tokens carry the scopes they were created with, and never more than their creator has. Root on the server itself uses the CLI without credentials through a Unix socket. This page covers all three kinds of caller.

## Users

The first account is created during setup, from the one-time link the installer prints. Print it again on the server with `sudo tond setup-link`. That first user is an admin.

Admins manage everyone else on the **Users** page:

- **Invite** a user with an email address and a role. Tond creates a one-time link, valid for 24 hours, where the new user picks their own password. Admins never choose passwords for other people.
- **Change a role**, or **disable** and re-enable an account. Both sign the user out everywhere.
- **Reset a password** by creating a new one-time link. On the server: `sudo tond users reset-link <email>`.
- **Delete** a user. You can't delete yourself or the last admin. Deleting a user deletes their API tokens.

Passwords must be 12 to 256 characters. There are no composition rules, but Tond refuses common and breached passwords (from a list built into the binary), a listed word padded with digits or symbols, repeated chunks and straight runs such as `123456789012`. Change your own password on your **Profile**.

## Roles

| Role | For | Scopes |
| --- | --- | --- |
| admin | Full control, including users, API tokens and server settings | Every scope |
| deployer | Shipping code: deploy, restart and roll back apps, manage previews and environment variables without reading values | `apps:read`, `apps:deploy`, `apps:control`, `apps:rollback`, `env:read-names`, `env:write`, `logs:read`, `metrics:read`, `events:read`, `cron:read`, `cron:trigger`, `previews:write`, `db:read`, `snapshots:read`, `snapshots:create` |
| viewer | Read-only access to apps, logs, metrics and activity, with no secret values | `apps:read`, `env:read-names`, `db:read`, `snapshots:read`, `logs:read`, `metrics:read`, `events:read`, `cron:read` |

A role value Tond doesn't recognise is treated as viewer, so a bad value can never grant more rights.

## Scopes

| Group | Scope | Allows |
| --- | --- | --- |
| Apps | `apps:read` | See apps, their settings, domains and deployment history |
| | `apps:deploy` | Deploy apps |
| | `apps:write` | Create apps and change their configuration, domains and volumes |
| | `apps:control` | Start, stop and restart apps |
| | `apps:rollback` | Roll an app back to an earlier deployment |
| | `apps:delete` | Delete apps and projects |
| Secrets | `env:read-names` | See environment variable names, not values |
| | `env:read-values` | Reveal environment variable values |
| | `env:write` | Add, change and remove environment variables |
| | `db:credentials` | Reveal and change database passwords and connection strings |
| Data | `db:read` | See databases and their status |
| | `db:write` | Create, start, stop and reconfigure databases |
| | `db:delete` | Delete databases |
| | `snapshots:read` | See snapshots and backup status |
| | `snapshots:create` | Take snapshots |
| | `snapshots:restore` | Restore and delete snapshots (overwrites live data) |
| Ops | `logs:read` | Read build, app, database and cron logs |
| | `metrics:read` | See metrics and alert rules |
| | `events:read` | See the activity (audit) log |
| | `cron:read` | See cron jobs and their runs |
| | `cron:write` | Create, change and delete cron jobs |
| | `cron:trigger` | Run a cron job now |
| | `previews:write` | Create and delete preview deployments |
| Admin | `settings:write` | Change server settings, integrations, alerts and backups |
| | `users:admin` | Invite, change and remove users and their sessions |
| | `tokens:admin` | See and revoke every user's API tokens |
| | `terminal:host` | Open terminal sessions (host shell and containers) |

Some actions reach the host and need an admin scope on top of the obvious one. For example, adding a volume or creating a Docker Compose app also needs `settings:write`. The concept pages list these under **Permissions**.

## Sessions

Signing in to the dashboard creates a session, stored in the `__Host-tond_session` cookie. Tond stores only a hash of the session token, so a leaked database or backup can't be replayed as a login.

| Setting | Default | Range |
| --- | --- | --- |
| Sign out after inactivity | 12 hours | 1 to 720 hours |
| Maximum session lifetime | 30 days | 1 to 365 days |

Admins change these in **Settings → Dashboard sessions**. A change applies to every session at once. You can see and revoke your own sessions on **Profile → Sessions**. Admins can revoke anyone's sessions from the **Users** page.

## Two-factor authentication

Turn on two-factor authentication on **Profile → Two-factor**. It uses a TOTP authenticator app. When you confirm it, Tond gives you 10 one-time backup codes and signs out your other sessions. At sign-in you then enter a code from the app or a backup code after your password. Each pending sign-in allows only a limited number of wrong codes. Turning two-factor off needs your password and a valid code. TOTP secrets are stored encrypted.

## API tokens

API tokens let scripts, CI and agents call the API. A token looks like `tond_pat_` followed by 64 hexadecimal characters. Tond stores only its SHA-256 hash. The token is shown once, when you create it. Lists show only the visible prefix (`tond_pat_` plus 8 characters), which secret scanners can also match.

Create tokens on **Profile → API tokens**, or on the server:

```sh
sudo tond token create --name ci --preset deployer --app web --expires 30d
```

Each token has:

| Property | Details |
| --- | --- |
| Kind | `personal`, `service`, `agent` or `ci` |
| Scopes | Chosen one by one, or from a preset |
| App or project filter | Optional: limit the token to some apps, or to the apps of some projects |
| Allowed IPs | Optional: IP addresses or CIDR ranges the token may be used from (up to 50) |
| Expiry | Default 90 days, or never. Agent tokens: default 24 hours, at most 30 days. |

Presets are starting points:

| Preset | Scopes |
| --- | --- |
| Read-only | Every read scope without secret values (the viewer role's scopes) |
| Deployer (CI) | `apps:read`, `apps:deploy`, `logs:read`. Pair it with an app filter. |
| Agent operator | `apps:read`, `apps:deploy`, `apps:control`, `apps:rollback`, `previews:write`, `env:read-names`, `logs:read`, `metrics:read`, `events:read` |

Rules that keep tokens safe:

- **A token never does more than its creator.** Scopes are checked against the creator's role when the token is created, and capped by the creator's *current* role on every request. Demoting a user narrows their tokens at once. Disabling a user suspends their tokens.
- **Agent tokens** must expire and can never hold admin scopes.
- **A limited token** (with an app or project filter) can only use the routes of its own apps and projects, plus the app and project lists, which show only what it may see. Every other route is refused.
- **Tokens can't create tokens**, so a leaked token can't mint more.
- Tokens created as root on the server have no owner (shown as "root (host)") and no ceiling beyond their scopes.

Tond records each token's last use (time and IP). Revoke a token from the dashboard or with `tond token revoke <id, name or prefix>`. Admins with `tokens:admin` see every user's tokens (`tond token list --all`).

To use the CLI from another machine, pass a server URL and a token:

```sh
export TOND_SERVER=https://tond.example.com
export TOND_TOKEN=tond_pat_...
tond apps list
```

See [CI with a deployer token](/guides/ci).

## The local socket

On the server, the `tond` CLI talks to tond-server over a Unix socket, `/opt/tond/data/tond.sock` by default. The socket's file mode (`0600`, owned by root) is the credential: whoever can open it is root on the host already. Requests on it need no password or token, get every scope, and are recorded in the audit log as `local-socket`.

Only the socket is trusted this way. A connection from `127.0.0.1` over TCP is never trusted, because tond-proxy forwards internet traffic to tond-server over loopback. No header can grant trust either. A few switches can only be flipped over the socket, such as turning the host terminal on or off (`sudo tond settings host-terminal on`).

## Related

- [Audit log](/concepts/audit-log)
- [Environment and secrets](/concepts/secrets)
- [Security hardening checklist](/guides/hardening)
- [Threat model](/security/threat-model)
- [CLI reference](/reference/cli)
