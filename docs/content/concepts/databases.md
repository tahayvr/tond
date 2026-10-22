# Databases

A managed database is a database server that Tond runs in its own Docker container on your server. Tond creates the credentials, keeps the data on disk, backs it up and connects it to your apps. Use a managed database when you want PostgreSQL, MySQL, MariaDB, Redis, KeyDB or MongoDB next to your apps without running it by hand.

## Engines and versions

A managed database is an engine, a major version and, for PostgreSQL, an optional variant. Together they pick a pinned image:

| Engine | Versions (default in bold) | Port in the container | Connection scheme |
| --- | --- | --- | --- |
| PostgreSQL (`postgres`) | 14, 15, **16**, 17, 18 (`postgres:<version>-alpine`) | 5432 | `postgres://` |
| MySQL (`mysql`) | 8.0, **8.4** (`mysql:<version>`) | 3306 | `mysql://` |
| MariaDB (`mariadb`) | 10.11, 11.4, **11.8** (`mariadb:<version>`) | 3306 | `mysql://` |
| Redis (`redis`) | 6, **7**, 8 (`redis:<version>-alpine`) | 6379 | `redis://` |
| KeyDB (`keydb`) | **latest** (`eqalpha/keydb:latest`) | 6379 | `redis://` |
| MongoDB (`mongodb`) | 6, **7**, 8 (`mongo:<version>`) | 27017 | `mongodb://` |

PostgreSQL has two variants, each available for versions 14 to 18:

| Variant | Image | Adds |
| --- | --- | --- |
| `pgvector` | `pgvector/pgvector:0.8.1-pg<version>` | Vector similarity search (`vector`) |
| `postgis` | `postgis/postgis:<version>-3.5-alpine` (3.6 for 17 and 18) | Spatial types and queries (`postgis`, `postgis_topology`, `fuzzystrmatch`, `postgis_tiger_geocoder`). x86_64 servers only. |

Pick the engine, variant and version when you create the database in the dashboard. `tond db create` uses the engine's default version and image. The version can only move forward later, through an [upgrade](#major-version-upgrades).

An admin can set a **custom image** on the database's page instead. It applies on the next **Redeploy**. A custom image runs with the database's data mounted, so it needs the `settings:write` scope. Tond doesn't manage the version of a database with a custom image, so it offers no upgrades for it.

## Create a database

Create one from **Databases → New database**, or with the CLI:

![The Databases page listing managed databases and their status](/img/databases/databases.light.webp)

```sh
tond db create shop-db --db-type postgres --memory-mb 512
```

The name uses letters, digits and hyphens. The dashboard also asks for **Automatic backups**, set to every day at 03:00 UTC by default (see [Backups](#backups)). Tond then:

1. generates credentials (see [Credentials](#credentials));
2. creates the data directory `<data>/databases/<name>` (by default under `/opt/tond/data`);
3. starts the container `tond-db-<name>` on the project network, or on the database's own network if it has no project;
4. publishes its port on a free host port from 5400 upward, bound to `127.0.0.1`.

Databases have no CPU limit. Without a memory limit, the default cap of 75% of the server's RAM applies, as for apps. To change the memory limit later, use the database's **Settings** tab.

## Credentials

Tond generates credentials when it creates the database:

| Value | Format |
| --- | --- |
| User | `tond_<name>` (hyphens become underscores) |
| Database name | `tond_<name>`. Redis and KeyDB have none. |
| Password | 24 random hexadecimal characters |
| Root password | A separate random password, for MySQL and MariaDB only |

For Redis and KeyDB, Tond writes an ACL configuration that turns off the default user. Clients must authenticate as the generated user, so a Redis on the network is never open to everyone.

Passwords are encrypted at rest with `TOND_SECRET_KEY` (see [Environment and secrets](/concepts/secrets)). API responses and the dashboard show them masked. To see them in full you need the `db:credentials` scope, which only admins have, and every reveal is recorded in the [audit log](/concepts/audit-log):

```sh
tond db credentials shop-db
```

To change a password, use the database's page. Tond runs the change inside the engine with the current credentials (`ALTER ROLE` in PostgreSQL, `ALTER USER` in MySQL and MariaDB, `changeUserPassword` in MongoDB, `ACL SETUSER` in Redis and KeyDB), and saves the new password only once the engine accepts it. The data is untouched and nothing restarts. For Redis and KeyDB, Tond also rewrites its generated ACL config, so a restart keeps the new password; their passwords can't contain spaces, quotes, backslashes or `#`. The database must be running. You can't rename the database or the user this way.

Apps keep their own copy of the password, usually in a connection string in their environment variables. After a change, Tond lists the apps whose environment still contains the old password and which variables hold it: update those values, then redeploy each app.

## Connection strings

Each database has up to three connection strings:

| String | Host | Use it from |
| --- | --- | --- |
| Internal | The database name, e.g. `shop-db:5432` | Apps on the same Docker network: project members and linked apps |
| Host | `localhost:<host port>` | The server itself, or your computer through an SSH tunnel |
| Public | The dashboard domain (or `<server-ip>`) and the host port | Anywhere, but only when public access is on |

An internal PostgreSQL string looks like this:

```text
postgres://tond_shop_db:<password>@shop-db:5432/tond_shop_db
```

Tond doesn't add the connection string to your apps for you. Copy the internal string into an environment variable of the app, such as `DATABASE_URL`. See [Environment and secrets](/concepts/secrets).

You can replace the internal or host string with your own value on the database's page, for example to add query parameters. These overrides are stored encrypted too.

## Public access

By default the database port listens on `127.0.0.1` only. **Enable public access** publishes it on all interfaces, so anyone on the internet can try to connect. Only admins can turn it on (`settings:write`). Turning public access on or off recreates the container at once, with the same data and host port.

> [!WARNING]
> Docker's firewall rules bypass UFW. A published port is reachable from the internet even if your firewall looks closed. Prefer an SSH tunnel to the host port, and turn on public access only when you need it.

### Allowed addresses

A public database can be limited to a list of **Allowed addresses**: IPv4 addresses or CIDR ranges such as `203.0.113.7` or `198.51.100.0/24`, up to 64. Connections from anywhere else are dropped before they reach the database. An empty list allows every address, and the database page warns about it.

Tond enforces the list in the kernel, in its own `TOND-DB` chain jumped to from Docker's `DOCKER-USER` chain, because UFW never sees traffic to a published port. The rules match only traffic that arrived on the published port, so apps on the database's Docker networks are never affected. Tond puts the rules back after a reboot and whenever a database changes. On a server without iptables or the `DOCKER-USER` chain (for example nftables-only hosts or rootless Docker), Tond refuses to save an allowlist and says why. The page shows **Enforced by the server firewall** when the rules are in place. Public databases are published on IPv4 only.

### TLS

PostgreSQL, MySQL and MariaDB can serve TLS: **Turn on TLS…** on the database page (admins). Tond mounts the Let's Encrypt certificate of the dashboard domain when there is one, since the public connection string uses that name, and a self-signed certificate otherwise. It refreshes the copy and restarts the database when the dashboard certificate is renewed. The public connection string then asks for encryption (`sslmode=require` for PostgreSQL, `ssl-mode=REQUIRED` for MySQL and MariaDB). TLS is offered, not forced: apps on the internal network can keep connecting without it. Turning TLS on or off recreates the container.

## Stop, start and redeploy

| Action | What it does |
| --- | --- |
| Stop | Stops the container. Apps using it lose their connection. |
| Start | Starts the existing container again |
| Redeploy | Removes the container and creates a new one with the same data, credentials, network and host port, using the custom image if one is set |

```sh
tond db stop shop-db
tond db start shop-db
```

Changing the memory limit or the image needs a **Redeploy** to take effect.

## PostgreSQL extensions

The **Extensions** section of a running PostgreSQL database's **Settings** tab lists the extensions its image offers and enables one with `CREATE EXTENSION`. Every image offers `pg_stat_statements`, `pg_trgm`, `pgcrypto`, `uuid-ossp`, `citext`, `hstore`, `btree_gin`, `btree_gist`, `unaccent` and `ltree`, plus the variant's own extensions. Other extensions are refused. An enabled extension stays until you run `DROP EXTENSION` yourself.

## Major-version upgrades

The **Version** section of the database's **Settings** tab offers the newer versions of the same engine and variant. Upgrades work for PostgreSQL, MySQL, MariaDB, Redis and MongoDB. KeyDB always runs its latest image, and downgrades are refused, because no engine reads data files written by a newer release. Only admins (`settings:write`) can upgrade, and the database must be running.

Engines can't simply start on an older version's data files, so Tond moves the data through a logical dump into a fresh data directory:

1. The database moves to a private network, so apps lose their connection and nothing is written after the dump.
2. Tond takes a backup of the database (shown as **Before an upgrade**). If it fails, nothing else happens.
3. A temporary container of the new version starts on an empty data directory with the same credentials, and the dump is loaded into it.
4. Tond switches over: the old data directory is kept under `<data>/db-upgrades/<name>/previous`, the new one takes its place, and the database is recreated on the new image with the same name, port, networks and credentials. If it doesn't pass a health check, the old data and version come back.
5. The upgrade waits for you. **Keep version …** deletes the old data directory. **Roll back to …** swaps it back, and anything written since the upgrade is lost.

A failure before the switch leaves the old version running on its untouched data. If tond-server restarts in the middle of an upgrade, it marks the upgrade failed and puts the old version back. To go back after you confirmed, restore the pre-upgrade backup into a new database.

## Delete a database

Deleting a database removes the container, its own network and **its data directory**: every table, key and file on its volume is gone. Existing snapshots are kept, so you can still restore from one. Deleting needs the `db:delete` scope, and the dashboard asks you to type the database's name.

```sh
tond db delete shop-db
```

## Backups

Every database appears in two kinds of snapshot:

- **Full snapshots** include every managed database.
- **Database backups** hold one database. Take one with **Back up now** on the database's **Backups** tab, or give the database a schedule of its own.

### Backup schedule

The **Backups** tab sets the database's own **Schedule**: off, a preset (every day at 03:00, every 6 hours, every hour, every Sunday at 03:00) or a custom five-field cron expression, evaluated in UTC. **Keep** sets how many scheduled backups of this database to keep, from 1 to 365 (default 7). The tab also shows the next and latest backup, and whether scheduled full snapshots cover the database too. If neither a schedule nor scheduled full snapshots protect a database, its page warns that nothing backs it up automatically.

A failed scheduled backup fires the **Backup failed** alert for that database. See [Metrics, uptime and alerts](/concepts/observability).

### What a backup holds

A backup contains a logical dump made through the engine, a raw copy of the data directory, and a description of the database (engine, version, variant):

| Engine | Dump |
| --- | --- |
| PostgreSQL | `pg_dump` archive |
| MySQL, MariaDB | `mysqldump` / `mariadb-dump` SQL file |
| MongoDB | `mongodump` archive |
| Redis, KeyDB | `SAVE`, then a copy of `dump.rdb` |

### Restore

Each backup on the **Backups** tab has two actions:

- **Restore…** replaces the database's data. Tond first takes a backup of the current data, so you can undo the restore. It then stops the container, swaps in the saved data directory, starts the container and imports the dump.
- **Restore to new…** creates a new database with the backup's data and leaves the original running and unchanged. The new database has the same engine, version and variant, new credentials and its own port. It lands in the original's project. Use it to inspect old data, recover a few rows, or go back to the version before an upgrade. If the restore fails, the new database is removed again.

Restoring into a new database also works from a full snapshot, and for a database that no longer exists, as long as the snapshot describes it (backups taken before this feature don't). Through the API it is `POST /api/v1/snapshots/{id}/restore-new-database` with the `source` database and the new `name`.

Scheduled backups of a database keep its own **Keep** count. Its other backups (manual, before a restore or an upgrade) keep as many as the full-snapshot retention count. If you set up remote storage, database backups are also uploaded, encrypted. See [Snapshots and backups](/concepts/backups).

## Logs and metrics

The database page has **Logs** (the container output) and **Metrics** (CPU, memory, network and processes, from the same collector as apps). See [Metrics, uptime and alerts](/concepts/observability).

## Permissions

| Action | Scope |
| --- | --- |
| See databases | `db:read` |
| Create, stop, start, redeploy, change memory or overrides | `db:write` |
| Set the backup schedule, enable extensions | `db:write` |
| Custom image, public access, allowed addresses, TLS | `db:write` and `settings:write` |
| Upgrade, keep or roll back a new version | `db:write` and `settings:write` |
| Restore a backup | `snapshots:restore` |
| Restore a backup into a new database | `snapshots:restore` and `db:write` |
| Reveal or change passwords | `db:credentials` |
| Delete | `db:delete` |

## Related

- [Networks and database links](/concepts/networks)
- [Migrate a database](/guides/migrate-database)
- [Snapshots and backups](/concepts/backups)
- [CLI reference](/reference/cli)
