// db.rs — database migration runner
//
// migrate() is called once at startup. It:
//   1. Enables WAL journal mode for better concurrent read throughput
//      (tond-proxy also reads the DB every 5 s from a separate process).
//   2. Runs all SQL files in crates/tond-server/migrations/ via the sqlx
//      embedded migrator. Files are embedded at compile time (no runtime
//      filesystem access required), discovered automatically in alphabetical
//      order, and tracked in the `_sqlx_migrations` table. Already-applied
//      migrations are skipped; modified ones are rejected with a checksum
//      error so schema drift is caught immediately on startup.
//
// To add a new migration: create the next numbered .sql file in migrations/.
// No changes to this file are needed.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    // Enable WAL mode for better concurrent performance.
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;

    // Discover, embed, and run all migrations from the migrations/ directory.
    sqlx::migrate!("./migrations").run(pool).await?;

    Ok(())
}
