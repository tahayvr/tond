# Databases

A managed database is a database server that Tond runs in its own Docker container on your server. Tond creates the credentials, keeps the data on disk, backs it up and connects it to your apps. Use a managed database when you want PostgreSQL, MySQL, MariaDB, Redis, KeyDB or MongoDB next to your apps without running it by hand.

## Engines

| Engine | Default image | Port in the container | Connection scheme |
| --- | --- | --- | --- |
| PostgreSQL (`postgres`) | `postgres:16-alpine` | 5432 | `postgres://` |
| MySQL (`mysql`) | `mysql:8` | 3306 | `mysql://` |
| MariaDB (`mariadb`) | `mariadb:11` | 3306 | `mysql://` |
| Redis (`redis`) | `redis:7-alpine` | 6379 | `redis://` |
| KeyDB (`keydb`) | `eqalpha/keydb:latest` | 6379 | `redis://` |
| MongoDB (`mongodb`) | `mongo:7` | 27017 | `mongodb://` |

An admin can set a **custom image** on the database's page, for example `postgres:15-alpine`. It applies on the next **Redeploy**. A custom image runs with the database's data mounted, so it needs the `settings:write` scope.

## Create a database

Create one from **Databases → New database**, or with the CLI:

![The Databases page listing managed databases and their status](/img/databases/databases.light.webp)

```sh
tond db create shop-db --db-type postgres --memory-mb 512
```

The name uses letters, digits and hyphens. Tond then:

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

To change a password, use the database's page. Tond runs the change inside the engine (for example `ALTER ROLE` in PostgreSQL) with the current credentials, and saves the new password only once the engine accepts it. The database must be running. You can't rename the database or the user this way.

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

## Delete a database

Deleting a database removes the container, its own network and **its data directory**: every table, key and file on its volume is gone. Existing snapshots are kept, so you can still restore from one. Deleting needs the `db:delete` scope, and the dashboard asks you to type the database's name.

```sh
tond db delete shop-db
```

## Backups

Every database appears in two kinds of snapshot:

- **Full snapshots** include every managed database.
- **Database backups** hold one database. Take one with **Back up now** on the database's **Backups** tab.

A backup contains a logical dump made through the engine, plus a raw copy of the data directory:

| Engine | Dump |
| --- | --- |
| PostgreSQL | `pg_dump` archive |
| MySQL, MariaDB | `mysqldump` / `mariadb-dump` SQL file |
| MongoDB | `mongodump` archive |
| Redis, KeyDB | `SAVE`, then a copy of `dump.rdb` |

**Restore…** on a backup replaces the database's data. Tond first takes a backup of the current data, so you can undo the restore. It then stops the container, swaps in the saved data directory, starts the container and imports the dump. Tond keeps as many backups per database as the full-snapshot retention count. If you set up remote storage, database backups are also uploaded, encrypted. See [Snapshots and backups](/concepts/backups).

## Logs and metrics

The database page has **Logs** (the container output) and **Metrics** (CPU, memory, network and processes, from the same collector as apps). See [Metrics, uptime and alerts](/concepts/observability).

## Permissions

| Action | Scope |
| --- | --- |
| See databases | `db:read` |
| Create, stop, start, redeploy, change memory or overrides | `db:write` |
| Custom image, public access | `db:write` and `settings:write` |
| Reveal or change passwords | `db:credentials` |
| Delete | `db:delete` |

## Related

- [Networks and database links](/concepts/networks)
- [Migrate a database](/guides/migrate-database)
- [Snapshots and backups](/concepts/backups)
- [CLI reference](/reference/cli)
