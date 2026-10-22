# Environment and secrets

Environment variables are how you configure an app: database URLs, API keys, feature flags. Tond treats every value as a secret. Values are encrypted on disk, masked in the dashboard and API, and removed from logs. All of this rests on one key, `TOND_SECRET_KEY`, which you need to back up.

## Environment variables

Each app has its own set of variables. Names use letters, digits and underscores and may not start with a digit (`DATABASE_URL`, `_PRIVATE`). Values are plain strings.

Edit them in the app's **Settings → Environment**. You can add variables one by one or paste a whole `.env` file with **Import from .env**. From the CLI:

```sh
tond apps env web DATABASE_URL=postgres://... LOG_LEVEL=info
```

How they behave:

- **Writes are patches.** Setting one variable never touches the others. Each change runs in its own transaction, so two people editing different keys don't overwrite each other.
- **Changes apply on the next deploy.** Values are stored at once, but a running container keeps the environment it started with. Deploy to apply them.
- **They are runtime only.** Tond passes them to the container when it starts. They aren't passed to the image build.
- **`PORT` is always set** to the app's container port.

## Project variables

A [project](/concepts/projects) can hold shared variables that every member gets. Tond layers them under each member's own:

1. the project's shared variables are the base;
2. the app's own variables go on top, so an app variable with the same name replaces the project's;
3. `PORT` is always the app's container port.

The same order applies to the app container, to `${VAR}` in a compose file and to cron jobs in the project (the job's own variables win). Previews follow their own rules on top of this (see below). Project variables are encrypted and removed from member apps' logs like app variables, and changes apply on each member's next deploy or cron run. Set them with the API (`PUT /api/v1/projects/{id}` with `env_vars`, which replaces the whole set; it needs the `env:write` scope).

Docker Compose apps take their environment from the compose file. Tond runs `docker compose` with a cleared environment, so `${VAR}` in the file can't read tond-server's own environment.

## Who can see values

Values are never returned in bulk. The variable list shows names with masked values (`••••••••`). Revealing one value is a separate request.

| Action | Scope | Roles |
| --- | --- | --- |
| See variable names | `env:read-names` | admin, deployer, viewer |
| Add, change and remove variables | `env:write` | admin, deployer |
| Reveal a value | `env:read-values` | admin |

A deployer can set a value but can't read it back. Every reveal is recorded in the [audit log](/concepts/audit-log). See [Users, roles and tokens](/concepts/access).

## Previews

Previews run pull-request code, so they don't inherit the app's environment. In **Settings → Environment** you choose which app variables previews may see, and you can add **preview overrides** that only previews get. Everything else is withheld, project variables included. A shared name is looked up in the same layered environment production gets, so sharing `SENTRY_DSN` passes the project's value when the app doesn't set its own, and a preview override replaces both. See [Previews](/concepts/previews).

## Secrets in logs

Before a line is stored or shown, Tond removes:

1. credentials in URLs (`https://user:token@host` becomes `https://***@host`);
2. the app's own secret values of 8 characters or more (environment variables, its project's shared variables, preview overrides, the webhook secret), including each line of a multi-line value such as a PEM key;
3. common token formats: AWS access key IDs, GitHub, GitLab, Slack and Tond tokens, and private key blocks.

Matches are replaced with `[redacted]`. This applies to deploy logs, container logs in the dashboard and API, and the excerpts shown for failed deploys. It lowers the risk of leaks but can't catch a secret the app transforms before printing it. Don't log secrets on purpose.

## Encryption at rest

Every secret Tond stores is encrypted with AES-256-GCM:

- app environment variables and preview overrides, project shared variables, and app webhook secrets;
- cron job environments;
- managed database passwords and connection-string overrides;
- GitHub App credentials, GitLab tokens and registry passwords;
- storage provider secret keys;
- notification channel secrets, the heartbeat URL and the audit shipping signing secret;
- users' two-factor secrets.

The encryption key is derived from `TOND_SECRET_KEY` with HKDF-SHA256. Each value is also bound to the column it belongs to, so a ciphertext copied into another field doesn't decrypt.

Passwords, API tokens and session tokens aren't encrypted. They are hashed and can't be read back at all.

## TOND_SECRET_KEY

The installer generates a random key (`openssl rand -hex 32`) and writes it to:

```text
/opt/tond/data/tond.env
```

The file is readable by root only (mode `0600`). systemd loads it into tond-server's environment. A release build of tond-server refuses to start without the key, and tells you how to create one.

### Back it up

Keep a copy of `TOND_SECRET_KEY` somewhere safe, off the server:

```sh
sudo cat /opt/tond/data/tond.env
```

The key is not part of Tond's snapshots. Without it:

- every encrypted value in the database (environment variables, database passwords, tokens, 2FA secrets) can't be recovered;
- snapshot copies in remote storage can't be decrypted, because their encryption key is derived from `TOND_SECRET_KEY`. Restoring on a new server needs the same key. See [Snapshots and backups](/concepts/backups).

> [!CAUTION]
> Losing the key has no workaround. Back it up right after you install Tond, and again after every rotation.

### Rotate it

To replace the key:

1. In `/opt/tond/data/tond.env`, rename the current `TOND_SECRET_KEY` line to `TOND_PREVIOUS_SECRET_KEY`.
2. Add a new `TOND_SECRET_KEY` (for example from `openssl rand -hex 32`).
3. Restart tond-server:

   ```sh
   sudo systemctl restart tond-server
   ```

4. Back up the new key.
5. Remove the `TOND_PREVIOUS_SECRET_KEY` line and restart again.

At startup, when `TOND_PREVIOUS_SECRET_KEY` is set, Tond re-encrypts every stored secret that only the previous key can read, in one transaction. This is safe to repeat. The log says how many values it re-encrypted, and warns about any value neither key can read.

While the previous key is set, Tond can still read snapshot archives made with it. Local snapshot copies are re-encrypted with the new key at startup (in the background). Remote archives and exports are not, and the database copy inside every snapshot taken before the rotation keeps its secrets sealed with the old key. Keep the old key safe for as long as you keep those snapshots. See [Snapshots and backups](/concepts/backups#key-rotation).

## Related

- [Users, roles and tokens](/concepts/access)
- [Snapshots and backups](/concepts/backups)
- [Configuration](/reference/configuration)
- [Security hardening checklist](/guides/hardening)
