# Migrate a database

This guide moves an existing PostgreSQL, MySQL (or MariaDB) or Redis database into a database managed by Tond, connects your app to it, and switches over. You need SSH access to the Tond server as root, a dump of the old database, and a few minutes of write downtime for the final switch.

Tond has no import button for outside dumps. You create an empty managed database, then load the dump into its container with the engine's own client over `docker exec`. The whole process runs on the server. (**Restore to new…** on a database's **Backups** tab only restores Tond's own backups.)

Managed databases run in a container named `tond-db-<name>`. You choose the major version when you create one:

| Engine | Versions (default in bold) |
| --- | --- |
| PostgreSQL | 14, 15, **16**, 17, 18 |
| MySQL | 8.0, **8.4** |
| MariaDB | 10.11, 11.4, **11.8** |
| Redis | 6, **7**, 8 |

Pick the same major version as the old database, or a newer one. Make the dump with a client that is no newer than the target's major version. A Redis RDB file from a newer Redis release won't load. If you start on the old version, you can [upgrade](/concepts/databases#major-version-upgrades) later from the database's **Settings** tab.

## 1. Create the managed database

In the dashboard, open **Databases → New database**, pick the engine, enter a **Database Name**, choose the **Version** (and, for PostgreSQL, a variant such as pgvector or PostGIS if the old database uses those extensions) and, if the app belongs to a project, the same **Project**. From the CLI:

```sh
tond db create shop-db --db-type postgres
```

The CLI always uses the engine's default version. Use `--project <project-id>` to put it in a project and `--memory-mb` to cap its memory.

Tond generates the user and password. The CLI prints the connection string once.

## 2. Get the connection details

```sh
tond db credentials shop-db
```

This prints the user, the password, the root password (MySQL and MariaDB) and two connection strings. Every reveal is recorded in the [audit log](/concepts/audit-log).

- **Conn** is the address from the server itself: `127.0.0.1` and a host port. The port is never published on the internet unless an admin turns on public access.
- **Internal** is the address apps with access use: the database's name as the host, for example `shop-db:5432`.

The database name and user are part of the connection string. In the dashboard, the same details are on the database page under **Overview → Connection**.

## 3. Make a dump and copy it to the server

On the old host, make a dump. For example:

```sh
# PostgreSQL: custom format, without ownership and grants from the old server
pg_dump --no-owner --no-privileges -Fc "$OLD_DATABASE_URL" > shop.dump

# MySQL / MariaDB
mysqldump --single-transaction --routines --triggers --no-tablespaces \
  -h old-db.example.com -u shop -p shop > shop.sql

# Redis: write a fresh snapshot, then copy the RDB file from the data directory
redis-cli SAVE
```

Copy the file to the Tond server:

```sh
scp shop.dump root@tond.example.com:/root/
```

## 4. Load the dump

Run these on the server. Replace the user, password and database name with the values from step 2. Passing the password as an environment variable keeps it out of the process list.

### PostgreSQL

For a custom-format dump:

```sh
docker exec -i -e PGPASSWORD='<password>' tond-db-shop-db \
  pg_restore -U <user> -d <dbname> --no-owner --no-privileges < /root/shop.dump
```

For a plain SQL dump:

```sh
docker exec -i -e PGPASSWORD='<password>' tond-db-shop-db \
  psql -U <user> -d <dbname> -v ON_ERROR_STOP=1 < /root/shop.sql
```

### MySQL and MariaDB

```sh
docker exec -i -e MYSQL_PWD='<password>' tond-db-shop-db \
  mysql -u <user> <dbname> < /root/shop.sql
```

On MariaDB the client is called `mariadb`. If the dump contains statements the application user may not run (for example `DEFINER` clauses on views or routines), import as `root` with the root password.

### Redis

Redis loads its data file at startup, so replace the file while the database is stopped:

```sh
tond db stop shop-cache
cp /root/dump.rdb /opt/tond/data/databases/shop-cache/dump.rdb
tond db start shop-cache
```

Stop first: Redis writes its own data file when it shuts down, which would overwrite yours. Tond's Redis requires the ACL user it generated; users and passwords from the old server are not carried over.

### Check the result

Open a shell in the database container and look around, for example:

```sh
docker exec -it -e PGPASSWORD='<password>' tond-db-shop-db psql -U <user> -d <dbname> -c '\dt'
```

Admins can also use the dashboard terminal and pick the `Database: shop-db` target. See [Host terminal](/guides/host-terminal).

If you prefer to use the database clients on your own machine, an SSH tunnel to the host port from step 2 works too: `ssh -L 5432:127.0.0.1:<host-port> root@tond.example.com`.

## 5. Give the app access

Each app and database runs on its own network. An app can reach a database when both are in the same project, or when the database is linked to the app:

- On the database page, under **Apps with access**, choose the app and click **Link app**, or
- On the app page, open **Settings → Databases** and click **Link database**.

Linking takes effect immediately and the app keeps running. Compose apps can't be linked. See [Networks and database links](/concepts/networks).

Then point the app at the new database with the **Internal** connection string, and redeploy:

```sh
tond apps env shop DATABASE_URL='postgres://<user>:<password>@shop-db:5432/<dbname>'
tond apps deploy shop
```

Environment changes apply on the next deploy. In the dashboard, use the app's **Settings → Environment**. See [Environment and secrets](/concepts/secrets).

## 6. Cut over

A trial import in steps 3 to 5 tells you how long the real one takes. For the final switch:

1. Stop writes to the old database: put the app in maintenance mode or stop it.
2. Make a final dump and load it into a fresh managed database. The simplest way to start clean is to delete the trial database in Tond and create it again with the same name; the new password comes from `tond db credentials`.
3. Update `DATABASE_URL` if the password changed, and deploy the app.
4. Check the app, then keep the old database, read-only, until you are sure.

## 7. Back it up

Take a first backup right away: on the database page, open **Backups** and click **Back up now**. On the same tab, check the database's **Schedule**: databases created in the dashboard are backed up every day at 03:00 UTC by default, while those created with the CLI have no schedule until you set one. See [Databases](/concepts/databases#backup-schedule). Off-server copies are covered in [Backups to S3](/guides/backups-s3).

## Next steps

- [Databases](/concepts/databases) explains managed databases, versions, upgrades, public access and custom images.
- [Snapshots and backups](/concepts/backups) covers restores, including restoring a backup into a new database.
