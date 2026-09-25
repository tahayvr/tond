// secret_key.rs — at-rest encryption for stored secrets
//
// Every secret tond-server stores encrypted (registry credential passwords,
// GitLab PATs, storage provider secret access keys, users' TOTP secrets) uses
// one format:
//
//   "v1." base64(nonce || AES-256-GCM ciphertext)
//
// The AES key is HKDF-SHA256(TOND_SECRET_KEY). Each value is bound to the
// column it belongs to through the GCM associated data (`Purpose`), so a
// ciphertext copied into another column does not decrypt.
//
// Release builds refuse to start without TOND_SECRET_KEY (`check_configured`);
// release/install.sh generates one into /opt/tond/data/tond.env, which
// tond-server.service loads. Debug builds fall back to a public dev secret.
//
// Key rotation: set TOND_PREVIOUS_SECRET_KEY to the old key and TOND_SECRET_KEY
// to the new one, then restart. At startup `rotate_stored_secrets` re-encrypts
// every value that only the previous key can read. It is idempotent and runs
// in one transaction.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Nonce, Payload},
};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;
use sqlx::{AssertSqlSafe, SqlitePool};

pub const SECRET_KEY_ENV: &str = "TOND_SECRET_KEY";
pub const PREVIOUS_SECRET_KEY_ENV: &str = "TOND_PREVIOUS_SECRET_KEY";

/// Secret used by debug builds when TOND_SECRET_KEY is unset. Public: never
/// acceptable in production, which `check_configured` enforces.
const DEV_SECRET: &str = "tond-dev-insecure-key-do-not-use-in-prod";

const PREFIX: &str = "v1.";
const NONCE_LEN: usize = 12;

/// What a ciphertext is for. Used as GCM associated data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    RegistryPassword,
    GitlabToken,
    StorageSecretKey,
    TotpSecret,
}

impl Purpose {
    fn aad(self) -> &'static [u8] {
        match self {
            Purpose::RegistryPassword => b"tond:registry_credentials.password_enc",
            Purpose::GitlabToken => b"tond:gitlab_tokens.token_enc",
            Purpose::StorageSecretKey => b"tond:storage_providers.secret_access_key_enc",
            Purpose::TotpSecret => b"tond:users.totp_secret_enc",
        }
    }
}

/// A 256-bit AES key derived from a secret.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key([u8; 32]);

impl Key {
    /// HKDF-SHA256 (RFC 5869) with a fixed salt and info; one 32-byte block.
    pub fn derive(secret: &str) -> Self {
        let mut extract = <Hmac<Sha256> as KeyInit>::new_from_slice(b"tond-secret-key-salt-v1")
            .expect("HMAC accepts any key length");
        extract.update(secret.as_bytes());
        let prk = extract.finalize().into_bytes();

        let mut expand =
            <Hmac<Sha256> as KeyInit>::new_from_slice(&prk).expect("HMAC accepts any key length");
        expand.update(b"tond stored secrets v1");
        expand.update(&[1]);
        Self(expand.finalize().into_bytes().into())
    }

    /// The key from TOND_SECRET_KEY (the dev secret when unset).
    pub fn current() -> Self {
        Self::derive(env_secret(SECRET_KEY_ENV).as_deref().unwrap_or(DEV_SECRET))
    }

    fn previous() -> Option<Self> {
        env_secret(PREVIOUS_SECRET_KEY_ENV).map(|s| Self::derive(&s))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.0).expect("key is 32 bytes")
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
        "{SECRET_KEY_ENV} is not set. Generate one with `openssl rand -hex 32`, write \
         `{SECRET_KEY_ENV}=<key>` to /opt/tond/data/tond.env (mode 0600) and restart tond-server."
    )
}

/// Encrypt `plaintext` for `purpose` with the current key.
pub fn encrypt(purpose: Purpose, plaintext: &str) -> Result<String> {
    encrypt_with(&Key::current(), purpose, plaintext)
}

/// Decrypt a value produced by `encrypt` for the same `purpose`.
pub fn decrypt(purpose: Purpose, encoded: &str) -> Result<String> {
    decrypt_with(&Key::current(), purpose, encoded)
}

pub fn encrypt_with(key: &Key, purpose: Purpose, plaintext: &str) -> Result<String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = key
        .cipher()
        .encrypt(
            &Nonce::<Aes256Gcm>::from(nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: purpose.aad(),
            },
        )
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", B64.encode(combined)))
}

pub fn decrypt_with(key: &Key, purpose: Purpose, encoded: &str) -> Result<String> {
    let body = encoded
        .strip_prefix(PREFIX)
        .context("unsupported ciphertext format")?;
    let combined = B64.decode(body).context("invalid base64 ciphertext")?;
    anyhow::ensure!(combined.len() > NONCE_LEN, "ciphertext too short");
    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes).context("invalid nonce length")?;
    let plaintext = key
        .cipher()
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: purpose.aad(),
            },
        )
        .map_err(|_| anyhow::anyhow!("decryption failed: wrong key or corrupted value"))?;
    String::from_utf8(plaintext).context("decrypted value is not UTF-8")
}

// ── Key rotation ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RotationReport {
    pub reencrypted: usize,
    pub undecryptable: usize,
}

/// Startup entry point: when TOND_PREVIOUS_SECRET_KEY is set, move every
/// stored secret it can read onto TOND_SECRET_KEY.
pub async fn rotate_stored_secrets(pool: &SqlitePool) -> Result<()> {
    let Some(previous) = Key::previous() else {
        return Ok(());
    };
    let report = reencrypt_stored_secrets(pool, &Key::current(), &previous).await?;
    tracing::info!(
        "Key rotation: re-encrypted {} stored secret(s) with the new {SECRET_KEY_ENV}. \
         Remove {PREVIOUS_SECRET_KEY_ENV} once you have a backup of the new key.",
        report.reencrypted
    );
    if report.undecryptable > 0 {
        tracing::error!(
            "Key rotation: {} stored secret(s) decrypt with neither key and must be re-entered.",
            report.undecryptable
        );
    }
    Ok(())
}

/// Re-encrypt one value: readable with `current` → unchanged; readable with
/// `previous` → re-encrypted with `current`; otherwise undecryptable.
fn rotate_value(
    value: &str,
    purpose: Purpose,
    current: &Key,
    previous: &Key,
    report: &mut RotationReport,
    what: &str,
) -> Result<Option<String>> {
    if decrypt_with(current, purpose, value).is_ok() {
        return Ok(None);
    }
    match decrypt_with(previous, purpose, value) {
        Ok(plaintext) => {
            report.reencrypted += 1;
            Ok(Some(encrypt_with(current, purpose, &plaintext)?))
        }
        Err(_) => {
            tracing::warn!("{what} cannot be decrypted with the current or previous key");
            report.undecryptable += 1;
            Ok(None)
        }
    }
}

/// Re-encrypt every stored secret that decrypts under `previous` but not
/// under `current`. Runs in a single transaction.
pub async fn reencrypt_stored_secrets(
    pool: &SqlitePool,
    current: &Key,
    previous: &Key,
) -> Result<RotationReport> {
    let mut report = RotationReport::default();
    if current == previous {
        return Ok(report);
    }
    let mut tx = pool.begin().await?;

    for (table, column, purpose) in [
        (
            "registry_credentials",
            "password_enc",
            Purpose::RegistryPassword,
        ),
        ("gitlab_tokens", "token_enc", Purpose::GitlabToken),
        ("users", "totp_secret_enc", Purpose::TotpSecret),
    ] {
        // `table` and `column` come from the literal list above: injection-safe.
        let rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT id, {column} FROM {table} WHERE {column} IS NOT NULL AND {column} != ''"
        )))
        .fetch_all(&mut *tx)
        .await?;
        for (id, value) in rows {
            let what = format!("{table}.{column} for id {id}");
            if let Some(new_value) =
                rotate_value(&value, purpose, current, previous, &mut report, &what)?
            {
                sqlx::query(AssertSqlSafe(format!(
                    "UPDATE {table} SET {column} = ? WHERE id = ?"
                )))
                .bind(&new_value)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
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
        let what = format!("storage_providers secret for id {id}");
        if let Some(new_value) = rotate_value(
            value,
            Purpose::StorageSecretKey,
            current,
            previous,
            &mut report,
            &what,
        )? {
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
    fn round_trips_and_rejects_wrong_key_purpose_and_garbage() {
        let a = Key::derive("key-a");
        let b = Key::derive("key-b");

        let enc = encrypt_with(&a, Purpose::GitlabToken, "hunter2").expect("encrypt");
        assert!(enc.starts_with("v1."));
        assert_eq!(
            decrypt_with(&a, Purpose::GitlabToken, &enc).ok().as_deref(),
            Some("hunter2")
        );
        assert!(
            decrypt_with(&b, Purpose::GitlabToken, &enc).is_err(),
            "wrong key"
        );
        assert!(
            decrypt_with(&a, Purpose::RegistryPassword, &enc).is_err(),
            "a ciphertext must not decrypt for another column"
        );
        assert!(decrypt_with(&a, Purpose::GitlabToken, "garbage").is_err());
        assert!(decrypt_with(&a, Purpose::GitlabToken, "v1.AAAA").is_err());
    }

    #[test]
    fn nonces_are_random() {
        let key = Key::derive("k");
        let one = encrypt_with(&key, Purpose::TotpSecret, "same").expect("encrypt");
        let two = encrypt_with(&key, Purpose::TotpSecret, "same").expect("encrypt");
        assert_ne!(one, two);
    }

    #[test]
    fn derivation_is_stable() {
        // Changing the KDF would make every stored secret unreadable. The vector
        // is HKDF-SHA256 computed independently (Python hmac/hashlib).
        assert_eq!(
            hex::encode(Key::derive("tond").0),
            "93590b65ffb1795ec89764df89df994009276d04ec9febca862726b357d564b6"
        );
        assert_ne!(Key::derive("tond").0, Key::derive("tond ").0);
    }

    #[tokio::test]
    async fn rotation_moves_secrets_to_new_key_and_is_idempotent() {
        let pool = test_pool().await;
        let old = Key::derive("old-production-secret");
        let new = Key::derive("new-production-secret");

        sqlx::query(
            "INSERT INTO registry_credentials (id, name, server, username, password_enc)
             VALUES ('rc1', 'hub', 'docker.io', 'me', ?)",
        )
        .bind(encrypt_with(&old, Purpose::RegistryPassword, "registry-pass").expect("encrypt"))
        .execute(&pool)
        .await
        .expect("insert registry credential");

        sqlx::query(
            "INSERT INTO gitlab_tokens (id, name, token_enc, gitlab_url, username)
             VALUES ('gl1', 'gl', ?, 'https://gitlab.com', 'me')",
        )
        .bind(encrypt_with(&old, Purpose::GitlabToken, "glpat-xyz").expect("encrypt"))
        .execute(&pool)
        .await
        .expect("insert gitlab token");

        let config = serde_json::json!({
            "region": "us-east-1",
            "bucket": "b",
            "access_key_id": "AKIA",
            "secret_access_key_enc": encrypt_with(&old, Purpose::StorageSecretKey, "s3-secret").expect("encrypt"),
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
        .bind(encrypt_with(&old, Purpose::TotpSecret, "JBSWY3DP").expect("encrypt"))
        .execute(&pool)
        .await
        .expect("insert user");

        // A row with garbage must be reported, not abort the rotation.
        sqlx::query(
            "INSERT INTO registry_credentials (id, name, server, username, password_enc)
             VALUES ('rc2', 'bad', 'docker.io', 'me', 'garbage')",
        )
        .execute(&pool)
        .await
        .expect("insert bad registry credential");

        let report = reencrypt_stored_secrets(&pool, &new, &old)
            .await
            .expect("rotation should succeed");
        assert_eq!(
            report,
            RotationReport {
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
            decrypt_with(&new, Purpose::RegistryPassword, &pw)
                .ok()
                .as_deref(),
            Some("registry-pass")
        );

        let (token,): (String,) =
            sqlx::query_as("SELECT token_enc FROM gitlab_tokens WHERE id = 'gl1'")
                .fetch_one(&pool)
                .await
                .expect("fetch gitlab token");
        assert_eq!(
            decrypt_with(&new, Purpose::GitlabToken, &token)
                .ok()
                .as_deref(),
            Some("glpat-xyz")
        );

        let (config,): (String,) =
            sqlx::query_as("SELECT config FROM storage_providers WHERE id = 'sp1'")
                .fetch_one(&pool)
                .await
                .expect("fetch storage provider");
        let config: serde_json::Value = serde_json::from_str(&config).expect("config json");
        assert_eq!(
            decrypt_with(
                &new,
                Purpose::StorageSecretKey,
                config["secret_access_key_enc"].as_str().expect("enc field")
            )
            .ok()
            .as_deref(),
            Some("s3-secret")
        );
        assert_eq!(config["access_key_id"], "AKIA", "other fields untouched");

        let (totp,): (String,) =
            sqlx::query_as("SELECT totp_secret_enc FROM users WHERE id = 'u1'")
                .fetch_one(&pool)
                .await
                .expect("fetch user");
        assert_eq!(
            decrypt_with(&new, Purpose::TotpSecret, &totp)
                .ok()
                .as_deref(),
            Some("JBSWY3DP")
        );

        let again = reencrypt_stored_secrets(&pool, &new, &old)
            .await
            .expect("second run should succeed");
        assert_eq!(
            again,
            RotationReport {
                reencrypted: 0,
                undecryptable: 1
            },
            "second run is a no-op apart from the undecryptable row"
        );
    }
}
