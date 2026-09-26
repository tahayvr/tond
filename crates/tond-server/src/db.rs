// db.rs — database pool setup and migration runner
//
// open_pool() builds the SQLite pool used by tond-server:
//   - journal_mode=WAL: readers never block the writer and vice versa. This
//     matters because tond-proxy reads the same file from another process
//     (route refresh every few seconds). WAL is persistent in the file, so the
//     proxy's own connections use it too.
//   - synchronous=NORMAL: in WAL mode this is still crash-safe for the
//     database (no corruption); a power loss can only drop the last few
//     commits. It avoids an fsync on every commit, which dominated write cost
//     on small VPS disks (every build line is a commit).
//   - a small pool: SQLite serialises writers anyway, and each connection
//     keeps its own page cache, so a handful of connections is plenty.
//   - statement logging: every statement is a DEBUG event (counted by
//     runtime::dbstats, never printed at RUST_LOG=info); statements slower
//     than dbstats::SLOW_THRESHOLD_MS are WARN (printed and counted as slow).
//   - change hooks on every connection (runtime::changes): background loops
//     and the proxy link wake when the tables they care about change instead
//     of polling.
//   - persistent WAL (SQLITE_FCNTL_PERSIST_WAL): closing the last connection
//     keeps tond.db-wal and tond.db-shm. tond-proxy opens the database
//     read-only and cannot create them, so they must never disappear (a
//     read-only connection cannot open a WAL database whose -wal is missing).
//
// migrate() is called once at startup. It runs all SQL files in
// crates/tond-server/migrations/ via the sqlx embedded migrator. Files are
// embedded at compile time, discovered in alphabetical order, and tracked in
// the `_sqlx_migrations` table. Already-applied migrations are skipped;
// modified ones are rejected with a checksum error so schema drift is caught
// immediately on startup.
//
// To add a new migration: create the next numbered .sql file in migrations/.
// No changes to this file are needed.
//
// share_with_proxy() keeps the database file at mode 0640. tond-proxy runs as
// the unprivileged `tond-proxy` user and reads tond.db through its group,
// which the file inherits from the setgid data directory (root:tond-proxy,
// set up by install.sh). SQLite gives -wal/-shm the same mode and owner, so
// the proxy can read all three and write none: it opens the database with
// mode=ro, maps -shm read-only, and everything it used to write (certificate
// state) now reaches tond-server over the control socket.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};

use crate::runtime::changes::HookState;
use crate::runtime::dbstats;

/// Begin a transaction that writes: `BEGIN IMMEDIATE` takes the write lock up
/// front, so a busy database waits out `busy_timeout`. A plain (deferred)
/// transaction that reads first and writes later can't wait: when another
/// connection commits in between, SQLite refuses the upgrade at once with
/// SQLITE_BUSY_SNAPSHOT (517, "database is locked"). Every tond-server
/// transaction writes, so use this instead of `pool.begin()`.
pub async fn begin_write(
    pool: &SqlitePool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Maximum pooled connections for tond-server.
const MAX_CONNECTIONS: u32 = 4;

/// Open the tond-server connection pool for the database file at `db_path`,
/// creating the file if it does not exist.
pub async fn open_pool(db_path: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .log_statements(log::LevelFilter::Debug)
        .log_slow_statements(
            log::LevelFilter::Warn,
            Duration::from_millis(dbstats::SLOW_THRESHOLD_MS),
        );

    let pool = SqlitePoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                let hooks = HookState::default();
                let mut handle = conn.lock_handle().await?;
                persist_wal(&mut handle);
                let on_change = hooks.clone();
                handle.set_update_hook(move |change| on_change.on_row_change(change.table));
                let on_commit = hooks.clone();
                handle.set_commit_hook(move || on_commit.on_commit());
                handle.set_rollback_hook(move || hooks.on_rollback());
                Ok(())
            })
        })
        .connect_with(options)
        .await?;
    share_with_proxy(Path::new(db_path));
    Ok(pool)
}

/// Keep -wal/-shm when this connection closes (see module docs).
fn persist_wal(handle: &mut sqlx::sqlite::LockedSqliteHandle<'_>) {
    let mut on: std::ffi::c_int = 1;
    // SAFETY: the handle is a live sqlite3 connection owned by sqlx for the
    // duration of the lock; SQLITE_FCNTL_PERSIST_WAL takes a pointer to an int
    // and only reads/writes that int.
    unsafe {
        libsqlite3_sys::sqlite3_file_control(
            handle.as_raw_handle().as_ptr(),
            c"main".as_ptr(),
            libsqlite3_sys::SQLITE_FCNTL_PERSIST_WAL,
            (&raw mut on).cast(),
        );
    }
}

/// Mode of tond.db, -wal and -shm: tond-server reads and writes, the
/// tond-proxy group only reads.
pub const DB_FILE_MODE: u32 = 0o640;

/// Set the database files to DB_FILE_MODE so tond-proxy can read them.
/// Best-effort: a missing file or a filesystem without Unix modes is ignored.
pub fn share_with_proxy(db_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = db_path.to_string_lossy();
    for file in [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-shm"),
    ] {
        let file = Path::new(&file);
        if file.is_file() {
            let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(DB_FILE_MODE));
        }
    }
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn share_with_proxy_gives_the_group_read_only_access() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let db = dir.path().join("tond.db");
        std::fs::write(&db, b"").unwrap_or_else(|err| panic!("write: {err}"));
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|err| panic!("chmod: {err}"));

        share_with_proxy(&db);

        let mode = std::fs::metadata(&db)
            .unwrap_or_else(|err| panic!("stat: {err}"))
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o640,
            "tond.db must be 0640: the proxy group reads, nobody else"
        );
        share_with_proxy(&dir.path().join("missing.db"));
    }

    #[tokio::test]
    async fn begin_write_waits_for_a_concurrent_writer_instead_of_failing() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
        let pool = open_pool(&dir.path().join("tond.db").to_string_lossy())
            .await
            .unwrap_or_else(|err| panic!("open_pool: {err}"));
        sqlx::query("CREATE TABLE t (n INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("create: {err}"));
        sqlx::query("INSERT INTO t (n) VALUES (0)")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));
        let bump = "UPDATE t SET n = n + 1";

        // A deferred transaction that reads, then sees another connection
        // commit, can't upgrade to a writer: SQLITE_BUSY_SNAPSHOT, at once.
        let mut deferred = pool
            .begin()
            .await
            .unwrap_or_else(|err| panic!("begin: {err}"));
        let _: i64 = sqlx::query_scalar("SELECT n FROM t")
            .fetch_one(&mut *deferred)
            .await
            .unwrap_or_else(|err| panic!("read: {err}"));
        sqlx::query(bump)
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("other writer: {err}"));
        let err = sqlx::query(bump)
            .execute(&mut *deferred)
            .await
            .err()
            .unwrap_or_else(|| panic!("a stale deferred transaction must not write"));
        assert!(err.to_string().contains("517"), "{err}");
        drop(deferred);

        // begin_write holds the write lock from the start: the other writer
        // waits (busy_timeout) and both writes land.
        let mut tx = begin_write(&pool)
            .await
            .unwrap_or_else(|err| panic!("begin_write: {err}"));
        let _: i64 = sqlx::query_scalar("SELECT n FROM t")
            .fetch_one(&mut *tx)
            .await
            .unwrap_or_else(|err| panic!("read: {err}"));
        let other = {
            let pool = pool.clone();
            tokio::spawn(async move { sqlx::query(bump).execute(&pool).await.map(|_| ()) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        sqlx::query(bump)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|err| panic!("write inside begin_write: {err}"));
        tx.commit()
            .await
            .unwrap_or_else(|err| panic!("commit: {err}"));
        other
            .await
            .unwrap_or_else(|err| panic!("join: {err}"))
            .unwrap_or_else(|err| panic!("the other writer must wait, not fail: {err}"));
        let n: i64 = sqlx::query_scalar("SELECT n FROM t")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|err| panic!("read: {err}"));
        assert_eq!(n, 3);
    }

    #[tokio::test]
    async fn open_pool_uses_wal_and_normal_synchronous() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
        let path = dir.path().join("tond.db");
        let pool = open_pool(&path.to_string_lossy())
            .await
            .unwrap_or_else(|err| panic!("open_pool should succeed: {err}"));

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|err| panic!("journal_mode: {err}"));
        assert_eq!(mode, "wal", "the server pool must run in WAL mode");

        // 1 = NORMAL
        let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|err| panic!("synchronous: {err}"));
        assert_eq!(sync, 1, "the server pool must use synchronous=NORMAL");

        migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("migrations should apply: {err}"));

        let file_mode = std::fs::metadata(&path)
            .unwrap_or_else(|err| panic!("stat: {err}"))
            .permissions()
            .mode();
        assert_eq!(
            file_mode & 0o777,
            0o640,
            "open_pool must leave tond.db readable by the proxy group only"
        );
    }

    #[tokio::test]
    async fn wal_files_survive_closing_the_pool() {
        let dir = tempfile::TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
        let path = dir.path().join("tond.db");
        let pool = open_pool(&path.to_string_lossy())
            .await
            .unwrap_or_else(|err| panic!("open_pool: {err}"));
        migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        pool.close().await;
        assert!(dir.path().join("tond.db-wal").exists(), "-wal must persist");
        assert!(dir.path().join("tond.db-shm").exists(), "-shm must persist");
    }

    #[tokio::test]
    async fn route_changes_bump_the_version_and_notify_watchers() {
        use crate::runtime::changes::{Table, subscribe};
        let dir = tempfile::TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
        let pool = open_pool(&dir.path().join("tond.db").to_string_lossy())
            .await
            .unwrap_or_else(|err| panic!("open_pool: {err}"));
        migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        let version = || async {
            sqlx::query_scalar::<_, i64>("SELECT version FROM route_state")
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|err| panic!("version: {err}"))
        };
        let v0 = version().await;
        let mut routes = subscribe(Table::RouteState);

        sqlx::query(
            "INSERT INTO apps (id, name, created_at, updated_at) VALUES ('a', 'web', 'now', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert: {err}"));
        assert_eq!(version().await, v0 + 1);
        assert!(
            routes.has_changed().unwrap_or(false),
            "the hook saw the trigger's write"
        );
        routes.mark_unchanged();

        // Health bookkeeping does not touch routing.
        sqlx::query("UPDATE apps SET health_consecutive_failures = 2 WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("update: {err}"));
        assert_eq!(version().await, v0 + 1);
        assert!(!routes.has_changed().unwrap_or(true));

        // pending -> building is not routable either; -> running is.
        sqlx::query("UPDATE apps SET status = 'building' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("update: {err}"));
        assert_eq!(version().await, v0 + 1);
        sqlx::query("UPDATE apps SET status = 'running', host_port = 8100, domain = 'web.example.com' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("update: {err}"));
        assert_eq!(version().await, v0 + 2);

        sqlx::query(
            "UPDATE settings SET value = 'dash.example.com' WHERE key = 'dashboard_domain'",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("update: {err}"));
        assert_eq!(version().await, v0 + 3);
        sqlx::query("INSERT INTO settings (key, value) VALUES ('server_name', 'x')")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));
        assert_eq!(version().await, v0 + 3, "unrelated settings don't count");
    }
}
