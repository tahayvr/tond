// secret_key.rs — at-rest encryption key for stored secrets
//
// Every secret tond-server stores encrypted (registry credential passwords,
// GitLab PATs, storage provider secret access keys, users' TOTP secrets) is
// encrypted with AES-256-GCM under a key derived from TOND_SECRET_KEY.
//
// Two ciphertext formats exist, each with its own key derivation:
//
//   base64(nonce || ciphertext), key = SHA-256(secret)
//     registry_credentials.password_enc, gitlab_tokens.token_enc and
//     storage_providers.config.secret_access_key_enc
//
//   hex(nonce) ":" hex(ciphertext), key = secret bytes, truncated/zero-padded to 32
//     users.totp_secret_enc
//
// When TOND_SECRET_KEY is unset each scheme falls back to a publicly known dev
// secret. Release builds refuse to start in that state (`check_configured`);
// release/install.sh and release/update.sh generate a key into
// /opt/tond/data/tond.env, which tond-server.service loads.
//
// Key migration (`reencrypt_stored_secrets`, run at startup after DB
// migrations): any stored value that does not decrypt under the current key
// but does decrypt under a previous one is re-encrypted with the current key.
// Previous keys are the dev fallbacks plus TOND_PREVIOUS_SECRET_KEY if set,
// so upgrading an install that ran without a key, or rotating the key, needs
// no manual step. The migration is idempotent and runs in one transaction.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Nonce},
};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, SqlitePool};

pub const SECRET_KEY_ENV: &str = "TOND_SECRET_KEY";
pub const PREVIOUS_SECRET_KEY_ENV: &str = "TOND_PREVIOUS_SECRET_KEY";

/// Fallback secret for the base64/SHA-256 scheme when TOND_SECRET_KEY is unset.
const DEV_FALLBACK_SECRET: &str = "tond-dev-insecure-key-do-not-use-in-prod";
/// Fallback secret for the TOTP scheme when TOND_SECRET_KEY is unset.
const TOTP_FALLBACK_SECRET: &str = "00000000000000000000000000000000";

/// The pair of AES keys derived from one secret (one per ciphertext format).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SecretKeys {
    /// Key for base64(nonce || ciphertext) values.
    pub aes: [u8; 32],
    /// Key for hex(nonce):hex(ciphertext) TOTP values.
    pub totp: [u8; 32],
}

impl SecretKeys {
    /// Derive keys from `secret`, or from the dev fallbacks when `None`.
    pub fn from_secret(secret: Option<&str>) -> Self {
        let aes = Sha256::digest(secret.unwrap_or(DEV_FALLBACK_SECRET).as_bytes()).into();

        let raw = secret.unwrap_or(TOTP_FALLBACK_SECRET).as_bytes();
        let mut totp = [0u8; 32];
        let len = raw.len().min(32);
        totp[..len].copy_from_slice(&raw[..len]);

        Self { aes, totp }
    }

    /// Keys derived from TOND_SECRET_KEY (or the dev fallbacks when unset).
    pub fn current() -> Self {
        Self::from_secret(env_secret(SECRET_KEY_ENV).as_deref())
    }
}

fn env_secret(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Fail in release builds when TOND_SECRET_KEY is unset; warn in debug builds.
pub fn check_configured() -> Result<()> {
    if env_secret(SECRET_KEY_ENV).is_some() {
        return Ok(());
    }
    if cfg!(debug_assertions) {
        tracing::warn!(
            "{SECRET_KEY_ENV} is not set — stored secrets are encrypted with the public dev \
             key. This is only acceptable for local development."
        );
        return Ok(());
    }
    anyhow::bail!(
        "{SECRET_KEY_ENV} is not set. Stored secrets would be encrypted with a publicly known \
         key. Generate one with `openssl rand -hex 32`, write `{SECRET_KEY_ENV}=<key>` to \
         /opt/tond/data/tond.env (mode 0600) and restart tond-server. Existing secrets are \
         re-encrypted automatically on the next start."
    )
}

// ── base64(nonce || ciphertext) format ─────────────────────────────────────

/// Encrypt `plaintext`. Returns base64(nonce || ciphertext).
pub fn encrypt(plaintext: &str, key: &[u8; 32]) -> Result<String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;

    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::<Aes256Gcm>::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(B64.encode(&combined))
}

/// Decrypt a value produced by [`encrypt`].
pub fn decrypt(encrypted: &str, key: &[u8; 32]) -> Result<String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;

    let combined = B64
        .decode(encrypted)
        .map_err(|e| anyhow::anyhow!("base64 decode failed: {e}"))?;

    if combined.len() < 12 {
        anyhow::bail!("encrypted payload too short");
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes)
        .map_err(|e| anyhow::anyhow!("invalid nonce: {e}"))?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| anyhow::anyhow!("utf8 decode failed: {e}"))
}

// ── hex(nonce):hex(ciphertext) format (TOTP secrets) ───────────────────────

/// Encrypt `plaintext`. Returns hex(nonce):hex(ciphertext).
pub fn encrypt_hex(plaintext: &str, key: &[u8; 32]) -> Result<String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;

    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::<Aes256Gcm>::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    Ok(format!(
        "{}:{}",
        hex::encode(nonce_bytes),
        hex::encode(ciphertext)
    ))
}

/// Decrypt a value produced by [`encrypt_hex`].
pub fn decrypt_hex(encoded: &str, key: &[u8; 32]) -> Option<String> {
    let (nonce_hex, ciphertext_hex) = encoded.split_once(':')?;
    let nonce_bytes = hex::decode(nonce_hex).ok()?;
    let ciphertext = hex::decode(ciphertext_hex).ok()?;
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes.as_slice()).ok()?;
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let plaintext = cipher.decrypt(&nonce, ciphertext.as_ref()).ok()?;
    String::from_utf8(plaintext).ok()
}

// ── Key migration ──────────────────────────────────────────────────────────

/// Counts of stored values touched by [`reencrypt_stored_secrets`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReencryptReport {
    /// Values re-encrypted from a previous key to the current key.
    pub reencrypted: usize,
    /// Values that decrypt under neither the current nor any previous key.
    pub undecryptable: usize,
}

enum Outcome {
    Current,
    Reencrypted(String),
    Undecryptable,
}

/// Re-encrypt one value: `decrypt` with the current key succeeds → leave it;
/// else the first previous key that works → re-encrypt with the current key.
fn reencrypt_value(
    value: &str,
    current: &[u8; 32],
    previous: &[[u8; 32]],
    decrypt: impl Fn(&str, &[u8; 32]) -> Option<String>,
    encrypt: impl Fn(&str, &[u8; 32]) -> Result<String>,
) -> Result<Outcome> {
    if decrypt(value, current).is_some() {
        return Ok(Outcome::Current);
    }
    for key in previous {
        if let Some(plaintext) = decrypt(value, key) {
            return Ok(Outcome::Reencrypted(encrypt(&plaintext, current)?));
        }
    }
    Ok(Outcome::Undecryptable)
}

/// Startup entry point: migrate stored secrets onto TOND_SECRET_KEY.
///
/// Does nothing when TOND_SECRET_KEY is unset (only reachable in debug builds).
pub async fn migrate_stored_secrets(pool: &SqlitePool) -> Result<()> {
    let Some(secret) = env_secret(SECRET_KEY_ENV) else {
        return Ok(());
    };
    let current = SecretKeys::from_secret(Some(&secret));

    let mut previous = Vec::new();
    if let Some(prev) = env_secret(PREVIOUS_SECRET_KEY_ENV) {
        previous.push(SecretKeys::from_secret(Some(&prev)));
    }
    previous.push(SecretKeys::from_secret(None));

    let report = reencrypt_stored_secrets(pool, &current, &previous).await?;
    if report.reencrypted > 0 {
        tracing::info!(
            "Re-encrypted {} stored secret(s) with the current {SECRET_KEY_ENV}",
            report.reencrypted
        );
    }
    if report.undecryptable > 0 {
        tracing::error!(
            "{} stored secret(s) could not be decrypted with {SECRET_KEY_ENV} or any previous \
             key; they must be re-entered. If the key was rotated, set \
             {PREVIOUS_SECRET_KEY_ENV} to the old key and restart.",
            report.undecryptable
        );
    }
    Ok(())
}

/// Re-encrypt every stored secret that decrypts under one of `previous` but
/// not under `current`. Runs in a single transaction.
pub async fn reencrypt_stored_secrets(
    pool: &SqlitePool,
    current: &SecretKeys,
    previous: &[SecretKeys],
) -> Result<ReencryptReport> {
    let previous: Vec<SecretKeys> = previous
        .iter()
        .copied()
        .filter(|keys| keys != current)
        .collect();
    let previous_aes: Vec<[u8; 32]> = previous.iter().map(|k| k.aes).collect();
    let previous_totp: Vec<[u8; 32]> = previous.iter().map(|k| k.totp).collect();

    let decrypt_b64 = |v: &str, k: &[u8; 32]| decrypt(v, k).ok();

    let mut report = ReencryptReport::default();
    let mut tx = pool.begin().await?;

    // Plain base64 columns.
    for (table, column) in [
        ("registry_credentials", "password_enc"),
        ("gitlab_tokens", "token_enc"),
    ] {
        // `table` and `column` come from the literal list above: injection-safe.
        let rows: Vec<(String, String)> =
            sqlx::query_as(AssertSqlSafe(format!("SELECT id, {column} FROM {table}")))
                .fetch_all(&mut *tx)
                .await?;
        for (id, value) in rows {
            match reencrypt_value(&value, &current.aes, &previous_aes, decrypt_b64, encrypt)? {
                Outcome::Current => {}
                Outcome::Reencrypted(new_value) => {
                    sqlx::query(AssertSqlSafe(format!(
                        "UPDATE {table} SET {column} = ? WHERE id = ?"
                    )))
                    .bind(&new_value)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                    report.reencrypted += 1;
                }
                Outcome::Undecryptable => {
                    tracing::warn!("{table}.{column} for id {id} cannot be decrypted");
                    report.undecryptable += 1;
                }
            }
        }
    }

    // storage_providers.config JSON → secret_access_key_enc.
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, config FROM storage_providers")
        .fetch_all(&mut *tx)
        .await?;
    for (id, config) in rows {
        let Ok(mut cfg) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&config)
        else {
            continue;
        };
        let Some(value) = cfg.get("secret_access_key_enc").and_then(|v| v.as_str()) else {
            continue;
        };
        match reencrypt_value(value, &current.aes, &previous_aes, decrypt_b64, encrypt)? {
            Outcome::Current => {}
            Outcome::Reencrypted(new_value) => {
                cfg.insert(
                    "secret_access_key_enc".to_string(),
                    serde_json::Value::String(new_value),
                );
                let new_config = serde_json::to_string(&cfg)
                    .context("failed to serialise storage provider config")?;
                sqlx::query("UPDATE storage_providers SET config = ? WHERE id = ?")
                    .bind(&new_config)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                report.reencrypted += 1;
            }
            Outcome::Undecryptable => {
                tracing::warn!("storage_providers secret for id {id} cannot be decrypted");
                report.undecryptable += 1;
            }
        }
    }

    // users.totp_secret_enc (hex format).
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, totp_secret_enc FROM users WHERE totp_secret_enc IS NOT NULL AND totp_secret_enc != ''",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (id, value) in rows {
        match reencrypt_value(
            &value,
            &current.totp,
            &previous_totp,
            decrypt_hex,
            encrypt_hex,
        )? {
            Outcome::Current => {}
            Outcome::Reencrypted(new_value) => {
                sqlx::query("UPDATE users SET totp_secret_enc = ? WHERE id = ?")
                    .bind(&new_value)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                report.reencrypted += 1;
            }
            Outcome::Undecryptable => {
                tracing::warn!("users.totp_secret_enc for user {id} cannot be decrypted");
                report.undecryptable += 1;
            }
        }
    }

    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for secret key tests: {err}"));
        pool
    }

    #[test]
    fn from_secret_matches_legacy_derivations() {
        let dev = SecretKeys::from_secret(None);
        let expected_aes: [u8; 32] =
            Sha256::digest(b"tond-dev-insecure-key-do-not-use-in-prod").into();
        assert_eq!(
            dev.aes, expected_aes,
            "dev fallback AES key must stay stable"
        );
        assert_eq!(
            dev.totp, [b'0'; 32],
            "dev fallback TOTP key must stay stable"
        );

        let short = SecretKeys::from_secret(Some("abc"));
        let mut expected_totp = [0u8; 32];
        expected_totp[..3].copy_from_slice(b"abc");
        assert_eq!(
            short.totp, expected_totp,
            "TOTP key is zero-padded raw bytes"
        );
    }

    #[test]
    fn both_formats_round_trip_and_reject_wrong_key() {
        let a = SecretKeys::from_secret(Some("key-a"));
        let b = SecretKeys::from_secret(Some("key-b"));

        let enc = encrypt("hunter2", &a.aes).expect("encrypt should succeed");
        assert_eq!(decrypt(&enc, &a.aes).ok().as_deref(), Some("hunter2"));
        assert!(decrypt(&enc, &b.aes).is_err(), "wrong key must not decrypt");

        let enc = encrypt_hex("JBSWY3DP", &a.totp).expect("encrypt_hex should succeed");
        assert_eq!(decrypt_hex(&enc, &a.totp).as_deref(), Some("JBSWY3DP"));
        assert_eq!(
            decrypt_hex(&enc, &b.totp),
            None,
            "wrong key must not decrypt"
        );
        assert_eq!(decrypt_hex("not-a-valid-payload", &a.totp), None);
    }

    #[tokio::test]
    async fn reencrypt_moves_dev_key_secrets_to_new_key_and_is_idempotent() {
        let pool = test_pool().await;
        let dev = SecretKeys::from_secret(None);
        let new = SecretKeys::from_secret(Some("a-real-production-secret"));

        sqlx::query(
            "INSERT INTO registry_credentials (id, name, server, username, password_enc)
             VALUES ('rc1', 'hub', 'docker.io', 'me', ?)",
        )
        .bind(encrypt("registry-pass", &dev.aes).expect("encrypt"))
        .execute(&pool)
        .await
        .expect("insert registry credential");

        sqlx::query(
            "INSERT INTO gitlab_tokens (id, name, token_enc, gitlab_url, username)
             VALUES ('gl1', 'gl', ?, 'https://gitlab.com', 'me')",
        )
        .bind(encrypt("glpat-xyz", &dev.aes).expect("encrypt"))
        .execute(&pool)
        .await
        .expect("insert gitlab token");

        let config = serde_json::json!({
            "region": "us-east-1",
            "bucket": "b",
            "access_key_id": "AKIA",
            "secret_access_key_enc": encrypt("s3-secret", &dev.aes).expect("encrypt"),
        });
        sqlx::query(
            "INSERT INTO storage_providers (id, name, kind, config) VALUES ('sp1', 's3', 's3', ?)",
        )
        .bind(config.to_string())
        .execute(&pool)
        .await
        .expect("insert storage provider");

        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled)
             VALUES ('u1', 'a@b.c', 'x', 'admin', '2026-01-01', '2026-01-01', ?, 1)",
        )
        .bind(encrypt_hex("JBSWY3DP", &dev.totp).expect("encrypt_hex"))
        .execute(&pool)
        .await
        .expect("insert user");

        // A row with garbage must be reported, not abort the migration.
        sqlx::query(
            "INSERT INTO registry_credentials (id, name, server, username, password_enc)
             VALUES ('rc2', 'bad', 'docker.io', 'me', 'garbage')",
        )
        .execute(&pool)
        .await
        .expect("insert bad registry credential");

        let report = reencrypt_stored_secrets(&pool, &new, &[dev])
            .await
            .expect("migration should succeed");
        assert_eq!(
            report,
            ReencryptReport {
                reencrypted: 4,
                undecryptable: 1
            }
        );

        let (pw,): (String,) =
            sqlx::query_as("SELECT password_enc FROM registry_credentials WHERE id = 'rc1'")
                .fetch_one(&pool)
                .await
                .expect("fetch registry credential");
        assert_eq!(
            decrypt(&pw, &new.aes).ok().as_deref(),
            Some("registry-pass")
        );

        let (token,): (String,) =
            sqlx::query_as("SELECT token_enc FROM gitlab_tokens WHERE id = 'gl1'")
                .fetch_one(&pool)
                .await
                .expect("fetch gitlab token");
        assert_eq!(decrypt(&token, &new.aes).ok().as_deref(), Some("glpat-xyz"));

        let (config,): (String,) =
            sqlx::query_as("SELECT config FROM storage_providers WHERE id = 'sp1'")
                .fetch_one(&pool)
                .await
                .expect("fetch storage provider");
        let config: serde_json::Value = serde_json::from_str(&config).expect("valid JSON");
        assert_eq!(config["bucket"], "b", "other config fields are preserved");
        let enc = config["secret_access_key_enc"].as_str().expect("string");
        assert_eq!(decrypt(enc, &new.aes).ok().as_deref(), Some("s3-secret"));

        let (totp,): (String,) =
            sqlx::query_as("SELECT totp_secret_enc FROM users WHERE id = 'u1'")
                .fetch_one(&pool)
                .await
                .expect("fetch user");
        assert_eq!(decrypt_hex(&totp, &new.totp).as_deref(), Some("JBSWY3DP"));

        let second = reencrypt_stored_secrets(&pool, &new, &[dev])
            .await
            .expect("second run should succeed");
        assert_eq!(
            second,
            ReencryptReport {
                reencrypted: 0,
                undecryptable: 1
            },
            "a second run must not touch already-migrated rows"
        );
    }
}
