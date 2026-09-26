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

use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Maximum pooled connections for tond-server.
const MAX_CONNECTIONS: u32 = 4;

/// Open the tond-server connection pool for the database file at `db_path`,
/// creating the file if it does not exist.
pub async fn open_pool(db_path: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect_with(options)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
