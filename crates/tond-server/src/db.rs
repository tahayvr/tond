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
//
// share_with_proxy() keeps the database file at mode 0660. tond-proxy runs as
// the unprivileged `tond-proxy` user and reaches tond.db through its group,
// which the file inherits from the setgid data directory (root:tond-proxy,
// set up by install.sh). SQLite gives -wal/-shm the same mode and owner.

use std::path::Path;

use anyhow::Result;
use sqlx::SqlitePool;

/// Set the database file to 0660 (owner + group) so tond-proxy can use it.
/// Best-effort: a missing file or a filesystem without Unix modes is ignored.
pub fn share_with_proxy(db_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if db_path.is_file() {
        let _ = std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o660));
    }
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    // Enable WAL mode for better concurrent performance.
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;

    // Discover, embed, and run all migrations from the migrations/ directory.
    sqlx::migrate!("./migrations").run(pool).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn share_with_proxy_sets_owner_and_group_read_write() {
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
            0o660,
            "tond.db must be 0660, not world-readable"
        );
        share_with_proxy(&dir.path().join("missing.db"));
    }
}
