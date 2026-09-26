// auth/setup_token.rs — one-time token that authorizes first-run admin setup
//
// Until the first account exists, anyone who can reach the dashboard could
// otherwise create the admin — and the dashboard domain shows up in
// Certificate Transparency logs minutes after the ACME order. So setup needs
// a token that only root on the host can read:
//
//   ensure_setup_token()  — at startup, with no users: make sure a token
//                           exists, store its SHA-256 hash, and write the token
//                           to <data dir>/setup-token (0600). With users: drop
//                           any leftover token and file.
//   consume()             — inside the setup transaction: delete the matching
//                           row; exactly one concurrent setup can succeed.
//   is_valid()            — lets the setup page check a link before showing
//                           the form; says nothing without the token.
//
// The token is never logged. install.sh prints the setup link built from the
// file, and `tond setup-link` prints it again.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tond_common::{generate_raw_key, hash_key};

use super::secret_file::{SETUP_TOKEN_FILE, data_dir, read_secret_file, write_secret_file};

/// Path of the setup token file in the data directory.
pub fn setup_token_path() -> PathBuf {
    data_dir().join(SETUP_TOKEN_FILE)
}

/// Make sure a setup token exists while no user does (see module docs).
pub async fn ensure_setup_token(pool: &SqlitePool, path: &Path) -> Result<()> {
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if users > 0 {
        sqlx::query("DELETE FROM setup_tokens").execute(pool).await?;
        remove_token_file(path);
        return Ok(());
    }

    // Keep the existing token across restarts so the link the installer printed
    // stays valid, as long as the file still matches the stored hash.
    if let Some(existing) = read_secret_file(path) {
        let known: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM setup_tokens WHERE token_hash = ?")
                .bind(hash_key(&existing))
                .fetch_optional(pool)
                .await?;
        if known.is_some() {
            log_waiting(path);
            return Ok(());
        }
    }

    let token = generate_raw_key();
    // Write the file first: if the DB insert then fails, the next start sees a
    // file without a matching hash and simply issues a new token.
    write_secret_file(path, &token)?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM setup_tokens")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO setup_tokens (token_hash, created_at) VALUES (?, ?)")
        .bind(hash_key(&token))
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    log_waiting(path);
    Ok(())
}

fn log_waiting(path: &Path) {
    tracing::info!(
        "No admin account yet. The one-time setup token is in {} (root only); \
         run `sudo tond setup-link` to print the setup link.",
        path.display()
    );
}

/// Whether `token` is a valid, unused setup token and setup is still open.
pub async fn is_valid(pool: &SqlitePool, token: &str) -> Result<bool, sqlx::Error> {
    let row: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM setup_tokens
         WHERE token_hash = ? AND NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(hash_key(token.trim()))
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Consume the setup token inside the setup transaction. Returns false when the
/// token is wrong or was already used (e.g. by a concurrent request).
pub async fn consume(tx: &mut Transaction<'_, Sqlite>, token: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM setup_tokens WHERE token_hash = ?")
        .bind(hash_key(token.trim()))
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Best-effort removal of the token file once setup is complete.
pub fn remove_token_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!("Removed used setup token file {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("Could not remove setup token file {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations: {err}"));
        pool
    }

    async fn token_rows(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM setup_tokens")
            .fetch_one(pool)
            .await
            .unwrap_or_else(|err| panic!("failed to count setup tokens: {err}"))
    }

    #[tokio::test]
    async fn first_start_writes_a_root_only_token_file_and_stores_only_its_hash() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join(SETUP_TOKEN_FILE);

        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("ensure_setup_token failed: {err}"));

        let token = read_secret_file(&path).unwrap_or_else(|| panic!("token file missing"));
        assert_eq!(token.len(), 64);
        let mode = std::fs::metadata(&path)
            .unwrap_or_else(|err| panic!("metadata: {err}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the setup token file must be 0600");

        let stored: String = sqlx::query_scalar("SELECT token_hash FROM setup_tokens")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to read setup token row: {err}"));
        assert_ne!(stored, token, "the raw token must not be stored");
        assert_eq!(stored, hash_key(&token));
        assert!(
            is_valid(&pool, &token)
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );
        assert!(
            !is_valid(&pool, "wrong")
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );
    }

    #[tokio::test]
    async fn restart_keeps_a_matching_token_and_replaces_a_stale_one() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join(SETUP_TOKEN_FILE);

        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        let first = read_secret_file(&path).unwrap_or_default();
        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            read_secret_file(&path).unwrap_or_default(),
            first,
            "a restart must keep the printed setup link valid"
        );

        write_secret_file(&path, "tampered").unwrap_or_else(|err| panic!("{err}"));
        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        let replaced = read_secret_file(&path).unwrap_or_default();
        assert_ne!(replaced, "tampered");
        assert_ne!(replaced, first);
        assert_eq!(token_rows(&pool).await, 1);
    }

    #[tokio::test]
    async fn existing_users_remove_any_leftover_token() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join(SETUP_TOKEN_FILE);
        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));

        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at)
             VALUES ('u1', 'a@example.com', 'x', 'admin', 'now', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));

        assert!(!path.exists(), "the token file must be removed once setup is done");
        assert_eq!(token_rows(&pool).await, 0);
    }

    #[tokio::test]
    async fn consume_succeeds_exactly_once() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join(SETUP_TOKEN_FILE);
        ensure_setup_token(&pool, &path)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        let token = read_secret_file(&path).unwrap_or_default();

        let mut tx = pool.begin().await.unwrap_or_else(|err| panic!("{err}"));
        assert!(
            consume(&mut tx, &token)
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );
        tx.commit().await.unwrap_or_else(|err| panic!("{err}"));

        let mut tx = pool.begin().await.unwrap_or_else(|err| panic!("{err}"));
        assert!(
            !consume(&mut tx, &token)
                .await
                .unwrap_or_else(|err| panic!("{err}")),
            "a used token must not be accepted again"
        );
    }
}
