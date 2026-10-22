# Move to a new server

This guide moves a whole Tond installation to another server: apps, managed databases, volumes, settings, users, domains and still-valid TLS certificates. It is also the procedure to rebuild a server that was lost, from its backups in remote storage.

The move is a full snapshot of the old server restored onto a fresh installation. Snapshots are explained in [Snapshots and backups](/concepts/backups).

## What you need

- **The old server's `TOND_SECRET_KEY`.** Snapshots are encrypted with it, and so are the secrets inside them (environment variables, database passwords, tokens). The new server must run with the same key. It is in `/opt/tond/data/tond.env` on the old server; keep a copy off the server, for example in your password manager.
- **One way to carry the snapshot across:** a storage provider both servers can reach (recommended, and the only option if the old server is gone), or a file you copy yourself.
- **A new server** that meets the [support matrix](/support-matrix), with Tond not yet in use. Anything already on it is replaced.

> [!WARNING]
> A restore replaces everything on the new server: its database (users included), apps, volumes and settings. Sign in afterwards with an account from the old server.

## 1. Put the old key on the new server

Before installing, put the saved `tond.env` in place. The installer keeps an existing key and never replaces it:

```sh
sudo mkdir -p /opt/tond/data
sudo install -m 600 -o root -g root tond.env /opt/tond/data/tond.env
```

Then install Tond as usual (see the [Quickstart](/quickstart)) and create a temporary admin account. It is replaced by the old server's accounts.

If Tond is already installed with another key, set `TOND_SECRET_KEY` in `/opt/tond/data/tond.env` to the old key and restart with `sudo systemctl restart tond-server`. A server with a different key lists the old snapshots as **Made with a different key**, and the CLI refuses them with `SNAPSHOT_KEY_MISMATCH`.

## 2a. Move through remote storage

On the old server, if it is still running, take a fresh snapshot and upload it:

```sh
tond export --remote                      # to the provider in the backup settings
tond export --remote --provider <id>      # or a given one
```

`tond export --remote` takes a full snapshot, uploads its encrypted archive with a manifest and waits until the upload finishes. If the old server is lost, use its latest scheduled snapshot instead: every remote copy has a manifest, so nothing else is needed.

On the new server, add the same storage provider (same endpoint, bucket, key prefix and keys) in **Integrations** → **Storage Providers**, or with `tond storage-provider create`. Then list what the bucket holds and restore:

```sh
tond snapshot remote --provider <id>
tond import <snapshot-id> --provider <id>
```

From the dashboard: open **Snapshots** → **Restore from storage**, choose the provider, click **Restore…** on the snapshot, choose **Everything** and type `restore` to confirm.

## 2b. Move with a file

On the old server:

```sh
sudo tond export --output /root/tond-move.tondsnap
```

This takes a full snapshot and writes it as one encrypted archive, plus `tond-move.tondsnap.manifest.json`, which lists what it holds, its size and its SHA-256. Copy both files to the new server (`scp`, `rsync`, a disk), check the checksum, and import:

```sh
sha256sum tond-move.tondsnap        # compare with the manifest
sudo tond import tond-move.tondsnap
```

`tond export` and `tond import` also work from another machine with `--server` and a token that has the `snapshots:create` and `snapshots:restore` scopes: the archive streams through the CLI.

## 3. What happens during the restore

1. The archive is downloaded (or uploaded), its checksum checked and every record authenticated. A wrong key or a corrupted archive stops here, and nothing on the server has changed.
2. A pre-restore snapshot of the new server is taken, then its containers are stopped.
3. Tond's database, app volumes, compose volumes, database data directories, deploy keys and the TLS certificates valid for at least another day are put in place. tond-server exits and systemd starts it again.
4. On start, tond-server restarts tond-proxy, recreates every managed database that was running, and redeploys every app that was running, through the build queue. Git apps are built again from their branch, image apps pulled again, and compose apps started from their stored compose file. Compose services with a backup hook then get their dump loaded.

Follow along with `tond apps list` and `journalctl -u tond-server -f`. The audit log records `snapshot_restore_finished` with what came back and anything that didn't. An app deployed from an upload has no source to rebuild from: deploy it again with `tond apps push`.

## 4. Switch traffic

Point your DNS records (the dashboard domain, app domains, the apps and preview wildcard domains) at the new server's address. Certificates restored from the old server keep working until they expire, and tond-proxy renews them as usual once DNS points at it. Certificates that were close to expiring are ordered again.

When everything runs on the new server, turn the old one off. Keep its last snapshot until you are sure.

## Related

- [Snapshots and backups](/concepts/backups)
- [Backups to S3](/guides/backups-s3)
- [Environment and secrets](/concepts/secrets)
- [CLI reference](/reference/cli)
