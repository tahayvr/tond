# Upgrading Tond

Tond updates itself from the dashboard or the CLI. Every update takes a full snapshot first, checks that the new version is healthy, and puts the old one back automatically if it isn't. For seven days afterwards you can roll back with one click.

You need the `settings:write` scope (admins have it). Anyone who can see the System page sees which version runs and whether a newer one exists.

## How you hear about new releases

Once a day Tond asks GitHub for the list of Tond releases. The check is folded into Tond's hourly maintenance pass, so it costs nothing extra; the request carries no information about your server beyond `tond/<version>` as its user agent. Updates are never installed on their own.

**System → Updates** shows the running version, the latest release and, when there is one, the release notes of every version since yours. **Check now** asks right away (at most once a minute). To stop the daily request, untick **Check for new releases every day**; you can still check by hand.

![System → Updates with a new release available and its notes](/img/upgrading/available.light.webp)

## Update from the dashboard

Click **Update to x.y.z** and confirm. The dialog spells out what happens:

![The update confirmation](/img/upgrading/confirm.light.webp)

The page then follows the update step by step, with its output:

1. **Snapshot**: a full snapshot of apps, databases and Tond's own data. It appears under Snapshots as *Before a Tond update*.
2. **Download and verify**: the release for your server's architecture, checked against its SHA-256 checksum and its minisign signature. A download that doesn't verify stops the update; nothing has changed yet.
3. **Back up**: the current binaries, dashboard and systemd units are copied to `/opt/tond/data/backups/tond_backup_<time>/`.
4. **Install**: `tond-server` stops, its database is copied into the same backup (nothing writes it while the server is stopped, so the copy is exact), and the new files are swapped in. Your apps and databases keep running; tond-proxy keeps serving them.
5. **Start and migrate**: the new `tond-server` starts and migrates its database. Tond waits up to 90 seconds for it to answer its health check and report the new version.
6. **Restart proxy**: tond-proxy restarts on the new version. Apps are unreachable for a few seconds.

![An update in progress](/img/upgrading/running.light.webp)

While `tond-server` restarts, the page says so and keeps trying; there's no need to reload. When the update succeeds, the page reloads itself to load the new dashboard.

## If an update fails

If the new version doesn't start, doesn't become healthy, or tond-proxy doesn't come back, Tond rolls back on its own: it stops everything, puts back the old binaries, dashboard and units, puts back the database copy when the new version may have migrated it, and starts the old version. The page (and the audit log) then says the update was rolled back, and why.

![An update that failed and was rolled back](/img/upgrading/rolled-back.light.webp)

Nothing is lost in a rollback: `tond-server` was stopped from the moment the database was copied until the rollback, so there were no writes to lose. The snapshot from step 1 is there as well.

In the rare case that putting the old version back fails too, the message says so and lists the files to copy back by hand:

```sh
sudo cp /opt/tond/data/backups/tond_backup_<time>/bin/* /opt/tond/bin/
sudo cp /opt/tond/data/backups/tond_backup_<time>/tond.db /opt/tond/data/
sudo systemctl start tond-server tond-proxy
```

`journalctl -u tond-update -u tond-server` has the details of what went wrong.

## Roll back to the previous version

For seven days after an update, **System → Updates** offers **Roll back to x.y.z**. Rolling back takes a full snapshot of the current state, installs the previous version from the update's backup, and starts it.

![The rollback offer after an update](/img/upgrading/rollback.light.webp)

> [!WARNING]
> If the update changed the database (most releases do), rolling back also restores the database from before the update: apps, deploys, settings and users changed since then are lost. The confirmation says when that applies, and the snapshot taken first keeps the newer state if you need something from it.

If the rollback itself fails, Tond puts the version you were on back.

## Update from the command line

`tond update` does exactly what the dashboard does and follows the progress in your terminal. Run it on the server as root, or from anywhere with `--server` and a token that has `settings:write`:

```sh
sudo tond update --check        # running and latest version, and the changelog
sudo tond update                # update to the latest release
sudo tond update --version 0.4.0
sudo tond update --rollback     # back to the version before the last update
```

It exits `0` when the update (or rollback) succeeded and non-zero otherwise, so it works in scripts. Add `--json` for the final status as JSON. `--skip-snapshot` skips the snapshot (not recommended).

### When tond-server is down

If `tond-server` doesn't answer, `tond update` warns and updates directly instead: it downloads and verifies the release itself and runs the release's `update.sh`. The same steps and the same automatic rollback apply, but no snapshot is taken first. You can ask for this path with `tond update --direct`, and preview it with `--dry-run`.

If the `tond` CLI itself is broken, fetch the updater from the web; it verifies the release the same way:

```sh
curl -fsSL https://tond.io/update | sudo bash
```

### update.sh

Every release bundle contains `update.sh`, which does the actual work (the dashboard and `tond update` run the new release's copy). Its options are listed in [Configuration](/reference/configuration#updatesh-flags) and its exit codes in [Exit codes](/reference/exit-codes#updatesh). Progress is written to `/opt/tond/data/update/status.json` and the output to `/opt/tond/data/update/update.log`.

## Good to know

- Only one update or rollback runs at a time.
- Tond keeps the three newest update backups in `/opt/tond/data/backups/`; older ones are deleted after a successful update. Snapshots follow your [snapshot retention](/concepts/backups).
- Updates and rollbacks are recorded in the [audit log](/concepts/audit-log) when they start and when they finish, with who asked for them.
- An update runs in its own systemd unit, `tond-update.service`, so it isn't interrupted when `tond-server` restarts.
- Updating skips versions freely: going from 0.3.0 straight to 0.6.0 applies every migration in between.
