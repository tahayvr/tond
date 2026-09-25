// auth.rs — API key lifecycle and root key bootstrapping
//
// This module is the top of the auth crate layer. It re-exports the two
// Axum extractors (RequireAuth, RateLimit) from their sub-modules so the
// rest of the codebase only needs to import from `crate::auth`.
//
// Two public async functions:
//
//   ensure_root_key(pool)
//     Called once at server startup. If the api_keys table is empty, it
//     generates a new root key via ApiKey::generate(), stores the hash in
//     the DB, and prints the plaintext key to stdout. The raw key is only
//     ever visible in this log line — it is never stored.
//
//   validate_key(pool, raw_key)
//     SHA-256 hashes the incoming raw key and looks it up in api_keys.
//     On a successful match, last_used_at is updated (best-effort, the
//     result is discarded). Returns Option<ApiKey>.
//     Called by the RequireAuth extractor on every Bearer-token request.

pub mod extractor;
pub mod rate_limit;

pub use extractor::{LocalPeer, LocalSocket, RequireAuth};
pub use rate_limit::{RateLimit, RateLimiter};

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use tond_common::{ApiKey, hash_key};

/// Ensure at least one API key exists. On first run, generate a root key
/// and print it to stdout for automation and recovery workflows.
pub async fn ensure_root_key(pool: &SqlitePool) -> Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys")
        .fetch_one(pool)
        .await?;

    if count.0 == 0 {
        let (api_key, raw_key) = ApiKey::generate("root");

        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&api_key.id)
        .bind(&api_key.name)
        .bind(&api_key.key_hash)
        .bind(&api_key.created_at)
        .bind(&api_key.last_used_at)
        .execute(pool)
        .await?;

        tracing::info!("================================================");
        tracing::info!("  ROOT API KEY (advanced / automation access)");
        tracing::info!("  {}", raw_key);
        tracing::info!("================================================");
        tracing::info!("Use the dashboard first to create your admin account.");
        tracing::info!(
            "This key is for CLI/API automation and can be recovered from the server logs."
        );
    } else {
        tracing::info!("API key(s) found, authentication enabled");
    }

    Ok(())
}

/// Validate a raw API key against the database. Returns the matching ApiKey
/// row if valid, or None if the key is invalid or not found.
pub async fn validate_key(pool: &SqlitePool, raw_key: &str) -> Result<Option<ApiKey>> {
    let key_hash = hash_key(raw_key);

    let api_key = sqlx::query_as::<_, ApiKey>("SELECT * FROM api_keys WHERE key_hash = ?")
        .bind(&key_hash)
        .fetch_optional(pool)
        .await?;

    // Update last_used_at on successful auth
    if let Some(ref key) = api_key {
        let now = Utc::now().to_rfc3339();
        let _ = sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&key.id)
            .execute(pool)
            .await;
    }

    Ok(api_key)
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for auth tests: {err}"));
        pool
    }

    #[tokio::test]
    async fn ensure_root_key_creates_single_root_key_when_table_is_empty() {
        let pool = test_pool().await;

        ensure_root_key(&pool).await.unwrap_or_else(|err| {
            panic!("ensure_root_key should succeed on an empty table: {err}")
        });

        let keys: Vec<ApiKey> = sqlx::query_as("SELECT * FROM api_keys")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to load api_keys after ensure_root_key: {err}"));

        assert_eq!(
            keys.len(),
            1,
            "ensure_root_key should create exactly one root key when no keys exist"
        );
        assert_eq!(
            keys[0].name, "root",
            "ensure_root_key should name the generated bootstrap key 'root'"
        );
    }

    #[tokio::test]
    async fn ensure_root_key_does_not_create_duplicate_key_when_one_exists() {
        let pool = test_pool().await;
        let (api_key, _) = ApiKey::generate("existing");

        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&api_key.id)
        .bind(&api_key.name)
        .bind(&api_key.key_hash)
        .bind(&api_key.created_at)
        .bind(&api_key.last_used_at)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed existing API key: {err}"));

        ensure_root_key(&pool).await.unwrap_or_else(|err| {
            panic!("ensure_root_key should succeed when keys already exist: {err}")
        });

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to count api_keys: {err}"));

        assert_eq!(
            count.0, 1,
            "ensure_root_key should not create additional keys when the table is already populated"
        );
    }

    #[tokio::test]
    async fn validate_key_returns_matching_key_and_updates_last_used_at() {
        let pool = test_pool().await;
        let (api_key, raw_key) = ApiKey::generate("root");

        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&api_key.id)
        .bind(&api_key.name)
        .bind(&api_key.key_hash)
        .bind(&api_key.created_at)
        .bind(&api_key.last_used_at)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed API key: {err}"));

        let validated = validate_key(&pool, &raw_key)
            .await
            .unwrap_or_else(|err| panic!("validate_key should succeed for a valid key: {err}"));

        assert!(
            validated.is_some(),
            "validate_key should return the matching ApiKey for a valid raw key"
        );
        let updated_last_used: Option<String> =
            sqlx::query_scalar("SELECT last_used_at FROM api_keys WHERE id = ?")
                .bind(&api_key.id)
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|err| panic!("failed to read updated last_used_at: {err}"));
        assert!(
            updated_last_used.is_some(),
            "validate_key should update last_used_at for successfully authenticated keys"
        );
    }

    #[tokio::test]
    async fn validate_key_returns_none_for_unknown_key() {
        let pool = test_pool().await;

        let validated = validate_key(&pool, "not-a-real-key")
            .await
            .unwrap_or_else(|err| panic!("validate_key should not error for unknown keys: {err}"));

        assert!(
            validated.is_none(),
            "validate_key should return None when the provided raw key does not match any stored hash"
        );
    }
}
