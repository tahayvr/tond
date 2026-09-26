# Snapshots and backups

A snapshot is a point-in-time copy of Tond's data on the server: its database, app volumes and managed databases. Snapshots can also be copied, encrypted, to S3-compatible storage off the server. You restore from them to undo a bad deploy, recover a database, or rebuild a server. This page explains what a snapshot contains, when Tond takes one, how long it keeps them and how restores work.

![The Snapshots page: schedule, retention, remote storage and the list of snapshots](/img/backups/snapshots.light.webp)

## Kinds of snapshot

| Kind | Scope | Taken | Contents |
| --- | --- | --- | --- |
| Full | `full` | By you, or on the schedule | Tond's SQLite database, every app volume, every managed database, SSH deploy keys |
| Pre-deploy | `app:<name>` | At the start of each deploy of an app with volumes | That app's volumes |
| Database backup | `database:<name>` | By you (**Back up now** on the database page) | One managed database |
| Pre-restore (safety net) | `full` or `database:<name>` | Automatically, before a full or database restore | What the restore is about to overwrite |

Take a full snapshot from the **Snapshots** page or with the CLI:

```sh
tond snapshot create --name "before upgrade"
tond snapshot list
tond snapshot list --app web
```

## How a snapshot is made

Local snapshots are directories under `<data>/backups/snapshots/<id>` (by default `/opt/tond/data/backups/…`):

- **SQLite.** Tond's database is copied with `VACUUM INTO`, which writes a consistent copy while tond-server and tond-proxy keep running.
- **App volumes.** Each directory under `<data>/volumes` is copied. Only regular files and directories are copied: symbolic links, sockets and devices are skipped, so a container can't make a snapshot read host files.
- **Managed databases.** Each database gets a logical dump through its engine (`pg_dump`, `mysqldump`/`mariadb-dump`, `mongodump`, or Redis `SAVE`) plus a raw copy of its data directory. If the dump fails, the raw copy is still taken.
- **Manifest.** A `manifest.json` records the Tond version, trigger, scope, sizes and what was included.

### Hard-link deduplication

Each snapshot is built against the previous one. A file whose size and modification time haven't changed is hard-linked instead of copied, so it takes no extra space. Each snapshot records two sizes: the bytes actually written, and the total logical size. A daily snapshot of a mostly unchanged server costs little more than what changed that day.

## Schedule and retention

Configure backups on the **Snapshots** page (**Backup settings**) or with `tond snapshot config`:

| Setting | Default |
| --- | --- |
| Scheduled backups | Off |
| Schedule (cron) | `0 3 * * *` (03:00 UTC every day) |
| Retain (full snapshots) | 7 |
| Pre-deploy retain (per app) | 10 |
| Keep local copy | On |
| Remote storage provider | None (local only) |

```sh
tond snapshot config show
tond snapshot config set --schedule "0 3 * * *" --retain 14 --pre-deploy-retain 5
```

The schedule accepts standard five-field cron expressions, evaluated in UTC, and macros such as `@daily`. Scheduled backups are off until you turn on **Enable scheduled backups** in the dashboard.

After each snapshot, Tond deletes the oldest ones beyond the limits, both the local directory and the remote copy:

- full snapshots: keep the newest *Retain* count;
- database backups: keep the same count per database;
- pre-deploy snapshots: keep the newest *Pre-deploy retain* count per app.

## Remote copies

With a storage provider selected, full snapshots and database backups are also uploaded to S3-compatible storage (AWS S3, Cloudflare R2, Backblaze B2, MinIO and others). Pre-deploy snapshots stay local. The upload runs in the background after the local copy is complete. Each snapshot's remote status shows whether it uploaded or why it failed.

- **Streamed.** The snapshot is packed with tar, compressed with gzip, encrypted and sent as a multipart upload in 8 MiB parts, so memory use stays constant whatever the size.
- **Encrypted.** Each archive uses AES-256-GCM with its own key, derived from `TOND_SECRET_KEY` and a random salt. Records are sealed in order, so a reordered, truncated or edited archive fails to decrypt. The storage provider never sees your data.
- **Checked.** Tond records the archive's SHA-256 at upload. A restore from remote storage downloads the whole archive, checks the checksum, verifies every record, and only then extracts anything.
- **Keep local copy off** deletes the local directory once the upload succeeds, to save disk. Restores then download the archive first.

> [!WARNING]
> Restoring a remote copy on another server needs the same `TOND_SECRET_KEY`. Back it up separately. It is not inside any snapshot. See [Environment and secrets](/concepts/secrets).

To set up a provider, see [Backups to S3](/guides/backups-s3).

## Restores

| Restore | Replaces | Safety net | Downtime |
| --- | --- | --- | --- |
| Full | Tond's database, all app volumes, all database data directories, deploy keys | A full pre-restore snapshot | tond-server restarts |
| App | One app's volume directory | None | The app's container stops briefly |
| Database | One database's data | A backup of that database | The database container stops briefly |

```sh
tond snapshot restore <snapshot-id>                       # full, asks for confirmation
tond snapshot rollback <snapshot-id> --app web            # one app's volumes
tond snapshot rollback <snapshot-id> --database shop-db   # one database
```

- **Full restore** works only from a full snapshot. Tond first takes a pre-restore snapshot, then verifies the snapshot, stops every app and database container, replaces the data, and exits so systemd restarts tond-server with the restored database.
- **App restore** stops the app's container, replaces `<data>/volumes/<app>` with the snapshot's copy and starts the container again. It needs a snapshot that contains the app's volumes: a full snapshot or one of the app's pre-deploy snapshots. It doesn't change the app's code or image. To go back to earlier code, [roll back the deployment](/concepts/deploys) as well.
- **Database restore** works from a full snapshot or a backup of that database. Tond first backs up the current data, and the restore fails without changing anything if that backup can't be made. It then stops the container, swaps in the saved data directory, starts the container and imports the logical dump. To undo the restore, restore the safety-net backup.

The dashboard asks you to confirm every restore before it overwrites data.

## Permissions

| Action | Scope |
| --- | --- |
| See snapshots and backup status | `snapshots:read` |
| Take a snapshot or database backup | `snapshots:create` |
| Restore or delete a snapshot | `snapshots:restore` |
| Change backup settings and storage providers | `settings:write` |

## Related

- [Backups to S3](/guides/backups-s3)
- [Databases](/concepts/databases)
- [Volumes](/concepts/volumes)
- [Upgrading Tond](/guides/upgrading)
- [CLI reference](/reference/cli)
