# Snapshots and backups

A snapshot is a point-in-time copy of Tond's data on the server: its database, app volumes and managed databases. Snapshots can also be copied, encrypted, to S3-compatible storage off the server. You restore from them to undo a bad deploy, recover a database, or rebuild a server. This page explains what a snapshot contains, when Tond takes one, how long it keeps them and how restores work.

![The Snapshots page: schedule, retention, remote storage and the list of snapshots](/img/backups/snapshots.light.webp)

## Kinds of snapshot

| Kind | Scope | Taken | Contents |
| --- | --- | --- | --- |
| Full | `full` | By you, or on the schedule | Tond's SQLite database, every app volume, every managed database, SSH deploy keys, TLS certificates |
| Before a Tond update | `full` | Automatically, before Tond updates itself or rolls back an update | Same as a full snapshot |
| Pre-deploy | `app:<name>` | At the start of each deploy of an app with volumes, while **Enable pre-deploy snapshots** is on (the default) | That app's volumes |
| Database backup | `database:<name>` | By you (**Back up now** on the database page), or on the database's own schedule | One managed database |
| Before an upgrade | `database:<name>` | Automatically, before a major-version upgrade of the database | One managed database |
| Pre-restore (safety net) | `full` or `database:<name>` | Automatically, before a full or database restore | What the restore is about to overwrite |

Each snapshot records its trigger: `manual`, `scheduled`, `pre_deploy`, `pre_restore`, `pre_update` (before a Tond update, labelled **Before a Tond update**) or `pre_upgrade` (before a database upgrade). A database's scheduled backups have the trigger `scheduled` and the scope `database:<name>`. See [Upgrading Tond](/guides/upgrading) and [Databases](/concepts/databases#major-version-upgrades).

Take a full snapshot from the **Snapshots** page or with the CLI:

```sh
tond snapshot create --name "before upgrade"
tond snapshot list
tond snapshot list --app web
```

## How a snapshot is made

Local snapshots are directories under `<data>/backups/snapshots/<id>` (by default `/opt/tond/data/backups/…`):

- **SQLite.** Tond's database is copied with `VACUUM INTO`, which writes a consistent copy while tond-server and tond-proxy keep running.
- **App volumes.** Each directory under `<data>/volumes` is copied, and so are the named volumes of compose apps. Only regular files and directories are copied: symbolic links, sockets and devices are skipped, so a container can't make a snapshot read host files.
- **Compose database services.** A service that declares a backup hook dumps itself first, and the dump is stored with the snapshot. See [Consistent backups of compose databases](#consistent-backups-of-compose-databases).
- **Managed databases.** Each database gets a logical dump through its engine (`pg_dump`, `mysqldump`/`mariadb-dump`, `mongodump`, or Redis `SAVE`) plus a raw copy of its data directory. If the dump fails, the raw copy is still taken.
- **TLS certificates** (full snapshots). The certificates and the ACME account, so a moved server serves HTTPS straight away.
- **Manifest.** A `manifest.json` records the Tond version, trigger, scope, sizes and what was included.

### Encrypted at rest

Every file of a local snapshot is encrypted on its own with AES-256-GCM, using a key derived from `TOND_SECRET_KEY` for local snapshot files. File and directory names stay readable, because they are app, volume and database names. Contents are not, so a copy of the backup directory, or a disk that leaves the server, doesn't give away your data. Files are encrypted in 64 KiB records as they are copied, so memory use stays flat whatever the size. Restores decrypt straight into place.

Snapshots taken before local copies were encrypted can't be read by this version. Delete them.

### Hard-link deduplication

Each snapshot is built against the previous one. A file whose size and modification time haven't changed is hard-linked instead of copied, so it takes no extra space. Encryption doesn't get in the way: an encrypted file's size follows from the original's, and it keeps the original's modification time. Each snapshot records two sizes: the bytes actually written, and the total logical size. A daily snapshot of a mostly unchanged server costs little more than what changed that day.

### Consistent backups of compose databases

Copying a running database's files gives, at best, what the database would find after a power cut. Managed databases are dumped through their engine for that reason. A database that runs inside a compose app (or a template) can do the same with two labels on its service:

| Label | Runs | Required |
| --- | --- | --- |
| `tond.backup.dump` | `sh -c` in the running service before its volumes are copied. Whatever it prints on standard output is stored, encrypted, as the service's dump. | To get a dump |
| `tond.backup.restore` | `sh -c` after an app restore has put the volumes back and started the service. It gets the dump on standard input. Tond retries it for up to three minutes while the database starts. | No |

```yaml
services:
  db:
    image: postgres:17-alpine
    labels:
      tond.backup.dump: 'pg_dump -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" --clean --if-exists'
      tond.backup.restore: 'psql -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" -q -v ON_ERROR_STOP=1'
```

The dump runs in addition to the volume copy. If it fails, the snapshot still has the raw files and records the failure in its manifest. A dump command may also write its output into a volume itself (Redis `SAVE` writes a consistent `dump.rdb`) and print nothing. Bundled templates with a Postgres or MySQL service carry these labels, and template validation requires a dump hook on every known database image that has a volume. See [Templates](/guides/templates).

## Schedule and retention

Configure backups on the **Snapshots** page (**Backup settings**) or with `tond snapshot config`:

| Setting | Default |
| --- | --- |
| Scheduled backups | Off |
| Schedule (cron) | `0 3 * * *` (03:00 UTC every day) |
| Retain (full snapshots) | 7 |
| Pre-deploy snapshots | On |
| Pre-deploy retain (per app) | 10 |
| Keep local copy | On |
| Remote storage provider | None (local only) |

```sh
tond snapshot config show
tond snapshot config set --schedule "0 3 * * *" --retain 14 --pre-deploy-retain 5
tond snapshot config set --pre-deploy-enabled false   # no snapshot before deploys
```

The schedule accepts standard five-field cron expressions, evaluated in UTC, and macros such as `@daily`. Scheduled backups are off until you turn on **Enable scheduled backups** in the dashboard.

These settings schedule full snapshots. Each managed database can also have a backup schedule of its own, with its own retention count, set on the database's **Backups** tab (see [Databases](/concepts/databases#backup-schedule)). The two are independent: a database schedule runs whether or not full snapshots are scheduled. When several are due at once, they run one after another.

### Missed schedules

Each schedule remembers the last time it ran. If the server was down when a full snapshot or a database backup was due, it runs once when Tond starts again, not once for every missed slot. The snapshot carries a note ("Catch-up: the backup due at … was missed while Tond was down"), the server log says so, and the audit log records `backup_catch_up`. Turning a schedule on, or changing its cron expression, starts counting from that moment, so it never triggers a catch-up by itself.

After each snapshot, Tond deletes the oldest ones beyond the limits, both the local directory and the remote copy:

- full snapshots (including those taken before a Tond update): keep the newest *Retain* count;
- a database's scheduled backups: keep the newest count set on that database (default 7);
- a database's other backups (manual, pre-restore, pre-upgrade): keep the newest *Retain* count per database;
- pre-deploy snapshots: keep the newest *Pre-deploy retain* count per app.

## Remote copies

With a storage provider selected, full snapshots and database backups are also uploaded to S3-compatible storage (AWS S3, Cloudflare R2, Backblaze B2, MinIO and others). Pre-deploy snapshots stay local. The upload runs in the background after the local copy is complete. Each snapshot's remote status shows whether it uploaded or why it failed.

- **Streamed.** The snapshot is packed with tar, compressed with gzip, encrypted and sent as a multipart upload in 8 MiB parts, so memory use stays constant whatever the size.
- **Encrypted.** Each archive uses AES-256-GCM with its own key, derived from `TOND_SECRET_KEY` and a random salt. Records are sealed in order, so a reordered, truncated or edited archive fails to decrypt. The storage provider never sees your data.
- **Checked.** Tond records the archive's SHA-256 at upload. A restore from remote storage downloads the whole archive, checks the checksum, verifies every record, and only then extracts anything.
- **Findable.** Next to each archive Tond stores a small manifest (`snapshots/<id>.manifest.json`) with the snapshot's date, size, checksum and Tond version. The names of what it holds (apps, databases, the snapshot's name) are sealed with `TOND_SECRET_KEY`, so the storage provider sees none of them. Any server with the same key can list the bucket and restore from it, including a fresh installation. See [Restore from storage](#restore-from-storage).
- **Keep local copy off** deletes the local directory once the upload succeeds, to save disk. Restores then download the archive first.

> [!WARNING]
> Restoring a remote copy on another server needs the same `TOND_SECRET_KEY`. Back it up separately. It is not inside any snapshot. See [Environment and secrets](/concepts/secrets).

### Key rotation

When you rotate `TOND_SECRET_KEY` (see [Environment and secrets](/concepts/secrets)), tond-server re-encrypts the local snapshot copies with the new key at startup, in the background, keeping the hard links between snapshots. Remote copies and exported archives are not rewritten, because that would mean downloading and uploading every one of them. Restores try the previous key too, so keep `TOND_PREVIOUS_SECRET_KEY` set (and a copy of the old key) until the old remote copies have aged out of retention, or take a new full snapshot right after rotating.

To set up a provider, see [Backups to S3](/guides/backups-s3).

## Restores

| Restore | Replaces | Safety net | Downtime |
| --- | --- | --- | --- |
| Full | Tond's database, all app volumes, all database data directories, deploy keys, valid TLS certificates | A full pre-restore snapshot | tond-server restarts, then apps and databases start again |
| App | One app's volumes (and its compose services' dumps, loaded by their restore hooks) | None | The app's containers stop briefly |
| Database | One database's data | A backup of that database | The database container stops briefly |
| Database, into a new one | Nothing: creates a new database | Not needed | None |

```sh
tond snapshot restore <snapshot-id>                       # full, asks for confirmation
tond snapshot rollback <snapshot-id> --app web            # one app's volumes
tond snapshot rollback <snapshot-id> --database shop-db   # one database
```

- **Full restore** works only from a full snapshot. Tond first takes a pre-restore snapshot, then verifies the snapshot, stops every app and database container, replaces the data, and exits so systemd restarts tond-server with the restored database.
- **App restore** stops the app's container, replaces `<data>/volumes/<app>` with the snapshot's copy and starts the container again. It needs a snapshot that contains the app's volumes: a full snapshot or one of the app's pre-deploy snapshots. It doesn't change the app's code or image. To go back to earlier code, [roll back the deployment](/concepts/deploys) as well.
- **Database restore** works from a full snapshot or a backup of that database. Tond first backs up the current data, and the restore fails without changing anything if that backup can't be made. It then stops the container, swaps in the saved data directory, starts the container and imports the logical dump. To undo the restore, restore the safety-net backup.
- **Restore into a new database** (**Restore to new…** on the database's **Backups** tab) creates a new managed database from a full snapshot or a database backup, with the same engine, version and variant, new credentials and its own port. The original keeps running untouched. See [Databases](/concepts/databases#restore).

- **After a full restore**, tond-server starts again on the restored database and brings everything back: each database and app that was running when the snapshot was taken is started, or recreated and redeployed through the build queue when its containers don't exist (a new server). Compose services then get their dumps loaded. TLS certificates that are still valid for at least a day are put back, and tond-proxy is restarted. The audit log records `snapshot_restore_finished` with what came back and what didn't.

The dashboard asks you to confirm every restore before it overwrites data, and asks you to type the name for restores that overwrite data.

### Restore from storage

A snapshot in a storage provider's bucket can be restored on any server that has the same `TOND_SECRET_KEY`, even one that never saw it. Add the storage provider, then open **Snapshots** → **Restore from storage**, or use the CLI:

![Restore from storage: the snapshots in a bucket, with what each one holds](/img/backups/restore-from-storage.light.webp)

```sh
tond snapshot remote                 # what the bucket holds (default provider)
tond import <snapshot-id> --yes      # add it here and restore everything
tond import <snapshot-id> --no-restore   # only add it to the snapshot list
```

Once added to the list, a snapshot restores like any other: everything, one app, or one database, in place or into a new database. A snapshot made with another key is listed as **Made with a different key** and refused with `SNAPSHOT_KEY_MISMATCH` before anything is downloaded. To move a whole server, see [Move to a new server](/guides/server-move).

## Permissions

| Action | Scope |
| --- | --- |
| See snapshots and backup status | `snapshots:read` |
| Take a snapshot or database backup | `snapshots:create` |
| Restore or delete a snapshot, import one, download an export | `snapshots:restore` |
| Upload a snapshot to storage now | `snapshots:create` |
| Restore a database into a new one | `snapshots:restore` and `db:write` |
| Set a database's backup schedule | `db:write` |
| Change backup settings and storage providers | `settings:write` |

## Related

- [Backups to S3](/guides/backups-s3)
- [Move to a new server](/guides/server-move)
- [Databases](/concepts/databases)
- [Volumes](/concepts/volumes)
- [Upgrading Tond](/guides/upgrading)
- [CLI reference](/reference/cli)
