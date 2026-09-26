// secret_key.rs — at-rest encryption for stored secrets
//
// Every secret tond-server stores is encrypted with one format: app env vars
// and preview overrides, project and cron env, app webhook secrets, managed
// database passwords and connection-string overrides, GitHub App credentials,
// GitLab PATs, notification integration secrets, registry
// passwords, storage provider secret keys and users' TOTP secrets:
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
use std::collections::BTreeMap;

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
    /// apps.env_vars (the whole JSON object, see `seal_map`)
    AppEnv,
    /// apps.preview_env_vars (the whole JSON object)
    AppPreviewEnv,
    AppWebhookSecret,
    /// projects.env_vars (the whole JSON object)
    ProjectEnv,
    /// cron_jobs.env (the whole JSON object)
    CronEnv,
    DatabasePassword,
    DatabaseRootPassword,
    /// databases.custom_external_url (may embed a password)
    DatabaseExternalUrl,
    /// databases.custom_internal_url (may embed a password)
    DatabaseInternalUrl,
    GithubClientSecret,
    GithubWebhookSecret,
    GithubPrivateKey,
    /// Secret fields inside integrations.config (bot tokens, webhook URLs,
    /// SMTP passwords, ntfy topics/tokens, Pushover/Gotify tokens, webhook
    /// signing secrets; see INTEGRATION_SECRET_FIELDS)
    IntegrationSecret,
    /// heartbeat_config.url_enc (a heartbeat URL is a bearer secret: anyone
    /// holding it can report the server healthy)
    HeartbeatUrl,
    /// audit_shipping.webhook_secret (HMAC key for shipped audit entries)
    AuditWebhookSecret,
    /// idempotency_keys.response (a stored response can hold a new token or
    /// database password). A cache: key rotation drops it instead.
    IdempotentResponse,
}

impl Purpose {
    fn aad(self) -> &'static [u8] {
        match self {
            Purpose::RegistryPassword => b"tond:registry_credentials.password_enc",
            Purpose::GitlabToken => b"tond:gitlab_tokens.token_enc",
            Purpose::StorageSecretKey => b"tond:storage_providers.secret_access_key_enc",
            Purpose::TotpSecret => b"tond:users.totp_secret_enc",
            Purpose::AppEnv => b"tond:apps.env_vars",
            Purpose::AppPreviewEnv => b"tond:apps.preview_env_vars",
            Purpose::AppWebhookSecret => b"tond:apps.webhook_secret",
            Purpose::ProjectEnv => b"tond:projects.env_vars",
            Purpose::CronEnv => b"tond:cron_jobs.env",
            Purpose::DatabasePassword => b"tond:databases.db_password",
            Purpose::DatabaseRootPassword => b"tond:databases.root_password",
            Purpose::DatabaseExternalUrl => b"tond:databases.custom_external_url",
            Purpose::DatabaseInternalUrl => b"tond:databases.custom_internal_url",
            Purpose::GithubClientSecret => b"tond:github_apps.client_secret",
            Purpose::GithubWebhookSecret => b"tond:github_apps.webhook_secret",
            Purpose::GithubPrivateKey => b"tond:github_apps.private_key",
            Purpose::IntegrationSecret => b"tond:integrations.config",
            Purpose::HeartbeatUrl => b"tond:heartbeat_config.url_enc",
            Purpose::AuditWebhookSecret => b"tond:audit_shipping.webhook_secret",
            Purpose::IdempotentResponse => b"tond:idempotency_keys.response",
        }
    }
}

/// HKDF-SHA256 (RFC 5869) of `secret` with `salt` and `info`; one 32-byte block.
pub fn hkdf_sha256(secret: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut extract =
        <Hmac<Sha256> as KeyInit>::new_from_slice(salt).expect("HMAC accepts any key length");
    extract.update(secret);
    let prk = extract.finalize().into_bytes();

    let mut expand =
        <Hmac<Sha256> as KeyInit>::new_from_slice(&prk).expect("HMAC accepts any key length");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

/// TOND_SECRET_KEY (the public dev secret when unset in debug builds).
pub fn current_secret() -> String {
    env_secret(SECRET_KEY_ENV).unwrap_or_else(|| DEV_SECRET.to_string())
}

/// TOND_PREVIOUS_SECRET_KEY, when a key rotation is in progress.
pub fn previous_secret() -> Option<String> {
    env_secret(PREVIOUS_SECRET_KEY_ENV)
}

/// A 256-bit AES key derived from a secret.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key([u8; 32]);

impl Key {
    /// HKDF-SHA256 (RFC 5869) with a fixed salt and info; one 32-byte block.
    pub fn derive(secret: &str) -> Self {
        Self(hkdf_sha256(
            secret.as_bytes(),
            b"tond-secret-key-salt-v1",
            b"tond stored secrets v1",
        ))
    }

    /// The key from TOND_SECRET_KEY (the dev secret when unset).
    pub fn current() -> Self {
        Self::derive(&current_secret())
    }

    fn previous() -> Option<Self> {
        previous_secret().map(|s| Self::derive(&s))
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

/// Encrypt an optional value; `None` stays `None`.
pub fn encrypt_opt(purpose: Purpose, plaintext: Option<&str>) -> Result<Option<String>> {
    plaintext.map(|p| encrypt(purpose, p)).transpose()
}

/// Decrypt an optional value; `None` stays `None`.
pub fn decrypt_opt(purpose: Purpose, encoded: Option<&str>) -> Result<Option<String>> {
    encoded.map(|e| decrypt(purpose, e)).transpose()
}

/// Stored form of an empty key/value map. It holds nothing secret, so it is
/// kept readable (column defaults use it too).
pub const EMPTY_MAP: &str = "{}";

/// Encrypt a key/value map (env vars) as one value: the JSON object is the
/// plaintext. An empty map is stored as `EMPTY_MAP`.
pub fn seal_map(purpose: Purpose, map: &BTreeMap<String, String>) -> Result<String> {
    if map.is_empty() {
        return Ok(EMPTY_MAP.to_string());
    }
    let json = serde_json::to_string(map).context("failed to serialise map")?;
    encrypt(purpose, &json)
}

/// Decrypt a map stored by `seal_map`. NULL, blank and `EMPTY_MAP` read as an
/// empty map; anything else must decrypt, so a corrupt value is an error and
/// is never silently replaced.
pub fn open_map(purpose: Purpose, stored: Option<&str>) -> Result<BTreeMap<String, String>> {
    let stored = stored.map(str::trim).unwrap_or_default();
    if stored.is_empty() || stored == EMPTY_MAP {
        return Ok(BTreeMap::new());
    }
    let json = decrypt(purpose, stored)?;
    let value: serde_json::Value =
        serde_json::from_str(&json).context("decrypted map is not valid JSON")?;
    let object = value
        .as_object()
        .context("decrypted map is not a JSON object")?;
    Ok(object
        .iter()
        .map(|(k, v)| {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (k.clone(), value)
        })
        .collect())
}

/// Encrypt the listed string fields of a JSON object in place (fields that
/// are missing, not strings or empty are left alone).
pub fn seal_json_fields(
    object: &mut serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
    purpose: Purpose,
) -> Result<()> {
    for field in fields {
        if let Some(serde_json::Value::String(value)) = object.get(*field)
            && !value.is_empty()
        {
            let sealed = encrypt(purpose, value)?;
            object.insert((*field).to_string(), serde_json::Value::String(sealed));
        }
    }
    Ok(())
}

/// Reverse of `seal_json_fields`.
pub fn open_json_fields(
    object: &mut serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
    purpose: Purpose,
) -> Result<()> {
    for field in fields {
        if let Some(serde_json::Value::String(value)) = object.get(*field)
            && !value.is_empty()
        {
            let opened = decrypt(purpose, value)
                .with_context(|| format!("stored secret '{field}' cannot be decrypted"))?;
            object.insert((*field).to_string(), serde_json::Value::String(opened));
        }
    }
    Ok(())
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

    for (table, column, purpose) in SCALAR_SECRET_COLUMNS {
        // `table` and `column` come from the literal list: injection-safe.
        // Empty maps are stored as readable `{}` and hold nothing to rotate.
        let rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT id, {column} FROM {table}
             WHERE {column} IS NOT NULL AND {column} != '' AND {column} != '{EMPTY_MAP}'"
        )))
        .fetch_all(&mut *tx)
        .await?;
        for (id, value) in rows {
            let what = format!("{table}.{column} for id {id}");
            if let Some(new_value) =
                rotate_value(&value, *purpose, current, previous, &mut report, &what)?
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

    // Secrets stored as fields inside a JSON config column.
    for (table, fields, purpose) in JSON_SECRET_FIELDS {
        let rows: Vec<(String, String)> =
            sqlx::query_as(AssertSqlSafe(format!("SELECT id, config FROM {table}")))
                .fetch_all(&mut *tx)
                .await?;
        for (id, config) in rows {
            let Ok(mut cfg) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&config)
            else {
                continue;
            };
            let mut changed = false;
            for field in *fields {
                let Some(value) = cfg.get(*field).and_then(|v| v.as_str()) else {
                    continue;
                };
                if value.is_empty() {
                    continue;
                }
                let what = format!("{table}.config.{field} for id {id}");
                if let Some(new_value) =
                    rotate_value(value, *purpose, current, previous, &mut report, &what)?
                {
                    cfg.insert((*field).to_string(), serde_json::Value::String(new_value));
                    changed = true;
                }
            }
            if changed {
                let new_config =
                    serde_json::to_string(&cfg).context("failed to serialise config")?;
                sqlx::query(AssertSqlSafe(format!(
                    "UPDATE {table} SET config = ? WHERE id = ?"
                )))
                .bind(&new_config)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }

    // Stored idempotent responses are a 24 h cache: drop them rather than
    // re-encrypt (a retry then simply runs again).
    sqlx::query("DELETE FROM idempotency_keys")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(report)
}

/// Every column that holds one encrypted value (a map sealed with `seal_map`
/// counts as one value). Key rotation walks this list: a new secret column
/// must be added here.
pub const SCALAR_SECRET_COLUMNS: &[(&str, &str, Purpose)] = &[
    ("heartbeat_config", "url_enc", Purpose::HeartbeatUrl),
    (
        "registry_credentials",
        "password_enc",
        Purpose::RegistryPassword,
    ),
    ("gitlab_tokens", "token_enc", Purpose::GitlabToken),
    ("users", "totp_secret_enc", Purpose::TotpSecret),
    ("apps", "env_vars", Purpose::AppEnv),
    ("apps", "preview_env_vars", Purpose::AppPreviewEnv),
    ("apps", "webhook_secret", Purpose::AppWebhookSecret),
    ("projects", "env_vars", Purpose::ProjectEnv),
    ("cron_jobs", "env", Purpose::CronEnv),
    ("databases", "db_password", Purpose::DatabasePassword),
    ("databases", "root_password", Purpose::DatabaseRootPassword),
    (
        "databases",
        "custom_external_url",
        Purpose::DatabaseExternalUrl,
    ),
    (
        "databases",
        "custom_internal_url",
        Purpose::DatabaseInternalUrl,
    ),
    ("github_apps", "client_secret", Purpose::GithubClientSecret),
    (
        "github_apps",
        "webhook_secret",
        Purpose::GithubWebhookSecret,
    ),
    ("github_apps", "private_key", Purpose::GithubPrivateKey),
    (
        "audit_shipping",
        "webhook_secret",
        Purpose::AuditWebhookSecret,
    ),
];

/// Secrets stored as string fields inside a `config` JSON column.
pub const JSON_SECRET_FIELDS: &[(&str, &[&str], Purpose)] = &[
    (
        "storage_providers",
        &["secret_access_key_enc"],
        Purpose::StorageSecretKey,
    ),
    (
        "integrations",
        tond_common::INTEGRATION_SECRET_FIELDS,
        Purpose::IntegrationSecret,
    ),
];

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

    #[tokio::test]
    async fn rotation_covers_every_new_secret_column() {
        let pool = test_pool().await;
        let old = Key::derive("old-production-secret");
        let new = Key::derive("new-production-secret");
        let enc = |purpose: Purpose, value: &str| {
            encrypt_with(&old, purpose, value).unwrap_or_else(|err| panic!("encrypt: {err}"))
        };

        sqlx::query(
            "INSERT INTO apps (id, name, created_at, updated_at, env_vars, preview_env_vars, webhook_secret)
             VALUES ('a1', 'web', 'now', 'now', ?, '{}', ?)",
        )
        .bind(enc(Purpose::AppEnv, r#"{"K":"v"}"#))
        .bind(enc(Purpose::AppWebhookSecret, "hook"))
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert app: {err}"));
        sqlx::query(
            "INSERT INTO databases (id, name, db_type, db_password, custom_external_url)
             VALUES ('d1', 'pg', 'postgres', ?, ?)",
        )
        .bind(enc(Purpose::DatabasePassword, "pw"))
        .bind(enc(Purpose::DatabaseExternalUrl, "postgres://u:pw@h/db"))
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert database: {err}"));
        sqlx::query(
            "INSERT INTO github_apps (id, name, app_id, client_id, client_secret, webhook_secret, private_key, slug)
             VALUES ('g1', 'gh', 1, 'cid', ?, ?, ?, 'slug')",
        )
        .bind(enc(Purpose::GithubClientSecret, "cs"))
        .bind(enc(Purpose::GithubWebhookSecret, "ws"))
        .bind(enc(Purpose::GithubPrivateKey, "pem"))
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert github app: {err}"));
        let config = serde_json::json!({
            "chat_id": "42",
            "bot_token": enc(Purpose::IntegrationSecret, "123:tok"),
        });
        sqlx::query(
            "INSERT INTO integrations (id, name, kind, config) VALUES ('i1', 'tg', 'telegram', ?)",
        )
        .bind(config.to_string())
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert integration: {err}"));
        let webhook = serde_json::json!({
            "webhook_url": enc(Purpose::IntegrationSecret, "https://hooks.example.com/x"),
            "signing_secret": enc(Purpose::IntegrationSecret, "sig"),
        });
        sqlx::query(
            "INSERT INTO integrations (id, name, kind, config) VALUES ('i2', 'hook', 'webhook', ?)",
        )
        .bind(webhook.to_string())
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("insert webhook integration: {err}"));

        let report = reencrypt_stored_secrets(&pool, &new, &old)
            .await
            .unwrap_or_else(|err| panic!("rotation: {err}"));
        assert_eq!(
            report,
            RotationReport {
                reencrypted: 10,
                undecryptable: 0
            },
            "every encrypted column must be rotated; the empty preview map is skipped"
        );

        let (env, hook): (String, String) =
            sqlx::query_as("SELECT env_vars, webhook_secret FROM apps WHERE id = 'a1'")
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|err| panic!("read app: {err}"));
        assert!(decrypt_with(&new, Purpose::AppEnv, &env).is_ok());
        assert_eq!(
            decrypt_with(&new, Purpose::AppWebhookSecret, &hook)
                .ok()
                .as_deref(),
            Some("hook")
        );
        let (config,): (String,) =
            sqlx::query_as("SELECT config FROM integrations WHERE id = 'i1'")
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|err| panic!("read integration: {err}"));
        let config: serde_json::Value =
            serde_json::from_str(&config).unwrap_or_else(|err| panic!("config json: {err}"));
        assert_eq!(
            decrypt_with(
                &new,
                Purpose::IntegrationSecret,
                config["bot_token"].as_str().unwrap_or_default()
            )
            .ok()
            .as_deref(),
            Some("123:tok")
        );
        assert_eq!(config["chat_id"], "42");
    }

    #[test]
    fn maps_round_trip_and_empty_maps_stay_readable() {
        let map = BTreeMap::from([("A".to_string(), "1".to_string())]);
        let sealed = seal_map(Purpose::AppEnv, &map).unwrap_or_default();
        assert!(sealed.starts_with("v1."));
        assert_eq!(open_map(Purpose::AppEnv, Some(&sealed)).ok(), Some(map));
        assert!(
            open_map(Purpose::ProjectEnv, Some(&sealed)).is_err(),
            "a map sealed for one column must not open for another"
        );
        assert_eq!(
            seal_map(Purpose::AppEnv, &BTreeMap::new()).ok().as_deref(),
            Some(EMPTY_MAP)
        );
    }
}
