# Backups to S3

This guide sends encrypted copies of your snapshots to S3-compatible storage (AWS S3, Cloudflare R2, Backblaze B2, MinIO, Wasabi and others), schedules them, and shows how to check and restore them. You need an admin account, a bucket, and an access key for it.

Snapshots themselves are explained in [Snapshots and backups](/concepts/backups). In short: a full snapshot holds Tond's database, app volumes and database dumps; a database backup holds one managed database. Both can be copied to remote storage. Pre-deploy snapshots of a single app stay on the server.

## Before you start: back up the secret key

Remote copies are encrypted with a key derived from `TOND_SECRET_KEY`, which lives in `/opt/tond/data/tond.env`. The same key encrypts every stored secret: environment variables, database passwords, tokens for integrations. Without it, remote copies can't be decrypted by anyone, including you.

Copy the file somewhere off the server now, for example into your password manager:

```sh
sudo cat /opt/tond/data/tond.env
```

Keep that copy apart from the bucket. A bucket plus its key in the same place protects against nothing.

## 1. Create a bucket and a key

In your storage provider, create a private bucket and an access key limited to it. The key needs to list, read, write and delete objects: Tond uploads snapshots, downloads them for restores and deletes the ones that fall out of retention.

## 2. Add the storage provider

In the dashboard, open **Integrations**, go to **Storage Providers** and click **Add Provider**. Fill in **Display Name**, **Endpoint URL**, **Region**, **Bucket**, optionally a **Key Prefix** (such as `tond/`), **Access Key ID** and **Secret Access Key**, then click **Save Provider** and **Test**.

From the CLI, on the server:

```sh
tond storage-provider create --name r2-backups \
  --endpoint https://<account-id>.r2.cloudflarestorage.com \
  --region auto --bucket tond-backups --prefix prod/ \
  --key-id <access-key-id> --secret <secret-access-key>
```

The command tests the connection right after creating the provider. Run the test again at any time with `tond storage-provider test <id>`; `tond storage-provider list` shows the IDs. The secret access key is encrypted at rest and never returned by the API.

## 3. Turn on scheduled snapshots

Open **Snapshots** and find **Backup settings**:

| Setting | Default | Meaning |
| --- | --- | --- |
| **Enable scheduled backups** | off | Take full snapshots on the schedule |
| **Schedule (cron)** | `0 3 * * *` | A cron expression; the default is daily at 03:00 |
| **Retain (full snapshots)** | 7 | How many full snapshots to keep; also how many backups to keep per database |
| **Enable pre-deploy snapshots** | on | Snapshot an app's volumes before each deploy |
| **Pre-deploy retain (per app)** | 10 | Pre-deploy snapshots kept per app |
| **Keep local copy** | on | Keep the snapshot on the server after uploading it |
| **Remote storage provider** | None — local only | Where to upload encrypted copies |

Choose the provider under **Remote storage provider**, turn on **Enable scheduled backups**, and save.

From the CLI, `tond snapshot config set` changes the schedule, retention, local copy and provider:

```sh
tond snapshot config set --provider <provider-id> --schedule "0 3 * * *" --retain 14
tond snapshot config show
```

The CLI has no switch for **Enable scheduled backups** itself. Turn it on in the dashboard, or with the API over the server's local socket:

```sh
curl -sf --unix-socket /opt/tond/data/tond.sock -X PUT http://localhost/api/v1/snapshots/config \
  -H 'Content-Type: application/json' -d '{"enabled": true}'
```

### How retention works

When a full snapshot falls out of **Retain (full snapshots)**, Tond deletes its local copy and its remote copy together. If you want a longer history in the bucket than on the server, keep that history with your storage provider (for example versioning or a lifecycle rule), not with a higher retention count on a small disk.

Turning **Keep local copy** off saves disk space: the snapshot is removed from the server once its upload succeeds, and restores download it again.

## 4. Take a snapshot and verify the remote copy

Take one now rather than waiting for the schedule:

```sh
tond snapshot create --name first-remote
tond snapshot list
```

The upload runs in the background after the local snapshot completes. In the snapshot list on the **Snapshots** page, a snapshot with a remote copy shows **Encrypted copy in remote storage**. A failed upload shows **Remote upload failed** with the error.

To be told about failures, create a **Backup failed** alert rule; it fires when the latest snapshot or its upload fails. See [Alerts and heartbeat](/guides/alerts).

Each upload records the archive's SHA-256. When Tond downloads a copy for a restore, it checks that checksum and authenticates every part of the archive before it extracts anything. A corrupted or replaced object is refused and nothing is restored.

The best test of a backup is a restore. A low-risk rehearsal is restoring one managed database: Tond first takes a safety-net backup of its current data, so you can go back.

## 5. Restore on the same server

- **Whole server**: on the **Snapshots** page, click **Restore…** on a full snapshot and type `restore` to confirm, or run `tond snapshot restore <snapshot-id>`. Tond takes a pre-restore snapshot first, replaces all data and restarts. The dashboard is unavailable for about a minute.
- **One app**: `tond snapshot rollback <snapshot-id> --app web` swaps the app's volumes back without restarting Tond.
- **One database**: `tond snapshot rollback <snapshot-id> --database shop-db`, or the **Backups** tab on the database page. A safety-net backup of the current data is taken first.

If the snapshot's local copy is gone, Tond downloads the remote copy, verifies it and restores from it.

## 6. Restoring on a new server

A remote copy can only be decrypted with the `TOND_SECRET_KEY` of the server that made it. To prepare a new server with the old key, put the saved `tond.env` in place before running the installer; the installer keeps an existing key and never replaces it:

```sh
sudo mkdir -p /opt/tond/data
sudo install -m 600 -o root -g root tond.env /opt/tond/data/tond.env
```

> [!WARNING]
> Tond restores remote copies only for snapshots recorded in its own database. A fresh installation starts with an empty snapshot list, and this version has no command to import snapshots from a bucket. Until it does, a remote copy can only be restored by the installation that made it: for example when **Keep local copy** is off, or the local snapshot directory was lost. It does not yet cover the loss of the whole server. Keep that in mind when you plan for disaster recovery.

## Next steps

- [Security hardening checklist](/guides/hardening) covers storing the key offline and the rest of a server's defences.
- [Retention](/operations/retention) lists how long other data is kept.
