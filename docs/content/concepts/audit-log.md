# Audit log

The audit log records who did what on your Tond server, and it is built so that nobody can quietly change that record. Every sign-in, every change and every reveal of a secret is written with the authenticated actor, and each entry is chained to the one before it by a hash. Use it to answer "who changed this, and when?", and ship it off the server when you need a copy an attacker can't rewrite.

## What is recorded

Tond records security-relevant events and every state change, for example:

- **Access:** `login`, `login_failed`, `logout`, `password_change`, `session_revoke`, `2fa_enabled`, `2fa_disabled`, `admin_setup`.
- **Users and tokens:** `user_invite`, `user_role_change`, `user_delete`, `token_create`, `token_revoke`.
- **Apps:** `create`, `deploy`, `rollback`, `stop`, `start`, `restart`, `delete`, `domain_update`, `volume_add`, `volume_remove`, `health_update`, `source_update`.
- **Secrets:** `env_reveal`, `env_update`, `db_credentials_reveal`, `webhook_secret_reveal`, `webhook_secret_rotate`.
- **Data:** `snapshot_create`, `snapshot_restore_full`, `snapshot_restore_app`, `snapshot_restore_database`, `db_backup`, database changes.
- **Server:** `settings_update`, `backup_config_update`, `retention_update`, integrations, alert rules, `terminal_opened` and `terminal_closed`, updates and rollbacks of Tond itself.

Each entry holds:

| Field | Meaning |
| --- | --- |
| `seq` | Position in the chain: 1, 2, 3 and so on, without gaps |
| `created_at` | When it happened |
| Actor | Kind (`user`, `token`, `local_socket`, `system` or `anonymous`), ID and label, such as an email address, a token name or `local-socket` |
| `action` | What was done |
| Resource | Type, ID and name of what it was done to |
| `outcome` | `success` or `failure` |
| `ip` | The client address, when there is one |
| `detail` | Extra context, e.g. `deployment=<id>` |
| `prev_hash`, `hash` | The chain links |

The actor always comes from how the request was authenticated, never from a field the client sends. Background work, such as the janitor or a scheduled backup, is recorded as `system`. A failed sign-in is recorded as `anonymous`, with the name it claimed and its address.

## Viewing it

The **Activity** page lists entries newest first. You can filter by actor, action, resource and outcome, and search. Reading it needs the `events:read` scope, which every role has. From the CLI:

```sh
tond audit log --limit 50
tond audit log --actor alice@example.com
tond audit log --action "login*"
```

A trailing `*` in `--action` matches a prefix. `--limit` accepts 1 to 200.

## How the chain works

Each entry stores:

```text
hash = sha256(prev_hash || canonical(entry))
```

`canonical` is a JSON array of the entry's fields in a fixed order, and `prev_hash` is the previous entry's hash. The first entry chains from a hash of all zeros. Editing, deleting or reordering any entry breaks every hash after it.

The database adds a second guard: SQLite triggers refuse any `UPDATE` of the audit log, and any `DELETE` except during retention pruning. Only tond-server writes the log, one entry at a time.

## Verifying

Check the whole chain from the **Activity** page, from the API (`GET /api/v1/audit-log/verify`), or on the server:

```sh
tond audit verify
```

On success it prints how many entries it checked and their range. If entries were pruned, it also says where the chain continues. If verification fails, it names the first broken entry and the problem, and exits with status 1, so you can run it from a monitoring script.

> [!NOTE]
> The chain detects changes *inside* the log. It can't detect that the newest entries were cut off, or that someone with root rewrote the whole chain, hashes included. For that, ship the log off the server.

## Retention

Audit entries are kept for 180 days by default. You can set 30 to 3650 days in **Settings** (retention). The janitor removes old entries from the start of the chain in one transaction. It records the last removed entry's `seq` and hash as a **checkpoint**, and appends an `audit_pruned` entry that names it. Verification starts from the checkpoint, so the retained chain still verifies, and the pruning is itself on the record.

## Shipping

Audit log shipping copies every entry, including `prev_hash` and `hash`, to a system your server can't rewrite. It is off by default. Set it up in **Settings → Audit log shipping** (`settings:write`). You can use one or both destinations:

| Destination | Format |
| --- | --- |
| Syslog server | RFC 5424 messages over TCP, framed by octet counting. TLS is on by default (port 6514), with the server's certificate checked against the system's trusted CAs. One message per entry, with the action as `MSGID` and the entry as JSON. |
| HTTPS webhook | `POST` of `{"type":"tond.audit","server":…,"entries":[…]}`, up to 200 entries per request, oldest first. Signed with `X-Tond-Timestamp` and `X-Tond-Signature: sha256=<HMAC-SHA256 of "<timestamp>.<body>">`. Any 2xx response counts as delivered. |

The audit log itself is the queue. Each destination remembers the last entry it accepted, so a restart loses nothing. A new or re-pointed destination starts from the oldest retained entry, so the remote copy holds the whole chain. Failed deliveries are retried with a delay that starts at 10 seconds and doubles up to 30 minutes. If retention prunes entries before a destination could receive them, Tond counts them as dropped. The settings page shows each destination's last success and last error, and has a test button.

To check the remote copy, recompute each shipped entry's hash from its `prev_hash` and fields, in `seq` order. Then compare the newest `seq` and hash with the server, which `tond audit verify` checks. An entry the remote copy has but the server lacks, or a different hash for the same `seq`, means the local log was rewritten.

## Related

- [Users, roles and tokens](/concepts/access)
- [Retention](/operations/retention)
- [Threat model](/security/threat-model)
- [CLI reference](/reference/cli)
