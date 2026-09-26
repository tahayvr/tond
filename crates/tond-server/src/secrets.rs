// secrets.rs — typed access to the encrypted columns
//
// Rows are loaded with their secret columns still encrypted (`secret_key`
// format). Code that needs a plaintext value — deploys, `docker exec`,
// webhook verification, notification dispatch, reveal endpoints — goes
// through the helpers here, so decryption happens only where it is needed and
// every write encrypts. See `secret_key::SCALAR_SECRET_COLUMNS` for the list
// of columns (key rotation walks the same list).

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use tond_common::{App, GitHubApp, INTEGRATION_SECRET_FIELDS, IntegrationConfig, ManagedDatabase};

use crate::secret_key::{self, Purpose};

// ── Apps ─────────────────────────────────────────────────────────────────────

/// The app's env vars (decrypted).
pub fn app_env(app: &App) -> Result<BTreeMap<String, String>> {
    secret_key::open_map(Purpose::AppEnv, app.env_vars.as_deref())
        .with_context(|| format!("cannot read the environment of app '{}'", app.name))
}

/// The app's preview-only env overrides (decrypted).
pub fn app_preview_env(app: &App) -> Result<BTreeMap<String, String>> {
    secret_key::open_map(Purpose::AppPreviewEnv, Some(&app.preview_env_vars))
        .with_context(|| format!("cannot read the preview overrides of app '{}'", app.name))
}

/// Stored form of an app env map.
pub fn seal_app_env(env: &BTreeMap<String, String>) -> Result<String> {
    secret_key::seal_map(Purpose::AppEnv, env)
}

/// Stored form of an app's preview overrides.
pub fn seal_app_preview_env(env: &BTreeMap<String, String>) -> Result<String> {
    secret_key::seal_map(Purpose::AppPreviewEnv, env)
}

/// The app's webhook secret (decrypted); `None` when it has none.
pub fn app_webhook_secret(app: &App) -> Result<Option<String>> {
    secret_key::decrypt_opt(Purpose::AppWebhookSecret, app.webhook_secret.as_deref())
        .with_context(|| format!("cannot read the webhook secret of app '{}'", app.name))
}

/// Stored form of an app webhook secret.
pub fn seal_webhook_secret(secret: &str) -> Result<String> {
    secret_key::encrypt(Purpose::AppWebhookSecret, secret)
}

/// Encrypt the secret columns of an app built in memory (e.g. `App::new`,
/// which carries a plaintext webhook secret and env) before it is inserted.
pub fn seal_new_app(app: &mut App, env: &BTreeMap<String, String>) -> Result<()> {
    app.env_vars = Some(seal_app_env(env)?);
    app.webhook_secret =
        secret_key::encrypt_opt(Purpose::AppWebhookSecret, app.webhook_secret.as_deref())?;
    Ok(())
}

// ── Projects and cron jobs ───────────────────────────────────────────────────

pub fn project_env(stored: &str) -> Result<BTreeMap<String, String>> {
    secret_key::open_map(Purpose::ProjectEnv, Some(stored))
}

pub fn seal_project_env(env: &BTreeMap<String, String>) -> Result<String> {
    secret_key::seal_map(Purpose::ProjectEnv, env)
}

pub fn cron_env(stored: Option<&str>) -> Result<BTreeMap<String, String>> {
    secret_key::open_map(Purpose::CronEnv, stored)
}

/// Stored form of a cron job env; `None` for an empty map.
pub fn seal_cron_env(env: &BTreeMap<String, String>) -> Result<Option<String>> {
    if env.is_empty() {
        return Ok(None);
    }
    secret_key::seal_map(Purpose::CronEnv, env).map(Some)
}

// ── Managed databases ────────────────────────────────────────────────────────

/// Decrypt a database row's passwords and connection-string overrides in
/// place. Call it right after loading, only where the plaintext is needed.
pub fn open_database(mut db: ManagedDatabase) -> Result<ManagedDatabase> {
    let what = || format!("cannot read the credentials of database '{}'", db.name);
    let db_password = secret_key::decrypt_opt(Purpose::DatabasePassword, db.db_password.as_deref())
        .with_context(what)?;
    let root_password =
        secret_key::decrypt_opt(Purpose::DatabaseRootPassword, db.root_password.as_deref())
            .with_context(what)?;
    let external = secret_key::decrypt_opt(
        Purpose::DatabaseExternalUrl,
        db.custom_external_url.as_deref(),
    )
    .with_context(what)?;
    let internal = secret_key::decrypt_opt(
        Purpose::DatabaseInternalUrl,
        db.custom_internal_url.as_deref(),
    )
    .with_context(what)?;
    db.db_password = db_password;
    db.root_password = root_password;
    db.custom_external_url = external;
    db.custom_internal_url = internal;
    Ok(db)
}

/// Encrypted copies of a database's secret columns, for INSERT/UPDATE.
pub struct SealedDatabaseSecrets {
    pub db_password: Option<String>,
    pub root_password: Option<String>,
    pub custom_external_url: Option<String>,
    pub custom_internal_url: Option<String>,
}

/// Encrypt the secret columns of a database whose fields hold plaintext.
pub fn seal_database(db: &ManagedDatabase) -> Result<SealedDatabaseSecrets> {
    Ok(SealedDatabaseSecrets {
        db_password: seal_db_password(db.db_password.as_deref())?,
        root_password: seal_db_root_password(db.root_password.as_deref())?,
        custom_external_url: secret_key::encrypt_opt(
            Purpose::DatabaseExternalUrl,
            db.custom_external_url.as_deref(),
        )?,
        custom_internal_url: secret_key::encrypt_opt(
            Purpose::DatabaseInternalUrl,
            db.custom_internal_url.as_deref(),
        )?,
    })
}

pub fn seal_db_password(password: Option<&str>) -> Result<Option<String>> {
    secret_key::encrypt_opt(Purpose::DatabasePassword, password)
}

pub fn seal_db_root_password(password: Option<&str>) -> Result<Option<String>> {
    secret_key::encrypt_opt(Purpose::DatabaseRootPassword, password)
}

pub fn seal_db_external_url(url: Option<&str>) -> Result<Option<String>> {
    secret_key::encrypt_opt(Purpose::DatabaseExternalUrl, url)
}

pub fn seal_db_internal_url(url: Option<&str>) -> Result<Option<String>> {
    secret_key::encrypt_opt(Purpose::DatabaseInternalUrl, url)
}

// ── GitHub Apps ──────────────────────────────────────────────────────────────

// `GitHubApp` rows always carry their credentials encrypted; these decrypt
// one field where it is used.

/// The App's RSA private key (PEM), for signing JWTs.
pub fn github_private_key(app: &GitHubApp) -> Result<String> {
    secret_key::decrypt(Purpose::GithubPrivateKey, &app.private_key)
        .with_context(|| format!("cannot read the private key of GitHub App '{}'", app.name))
}

/// The App's webhook secret, for verifying deliveries.
pub fn github_webhook_secret(app: &GitHubApp) -> Result<String> {
    secret_key::decrypt(Purpose::GithubWebhookSecret, &app.webhook_secret).with_context(|| {
        format!(
            "cannot read the webhook secret of GitHub App '{}'",
            app.name
        )
    })
}

/// Encrypted (client_secret, webhook_secret, private_key) for an INSERT.
pub fn seal_github_credentials(
    client_secret: &str,
    webhook_secret: &str,
    private_key: &str,
) -> Result<(String, String, String)> {
    Ok((
        secret_key::encrypt(Purpose::GithubClientSecret, client_secret)?,
        secret_key::encrypt(Purpose::GithubWebhookSecret, webhook_secret)?,
        secret_key::encrypt(Purpose::GithubPrivateKey, private_key)?,
    ))
}

// ── Notification integrations ────────────────────────────────────────────────

type JsonObject = serde_json::Map<String, serde_json::Value>;

fn parse_object(json: &str) -> Result<JsonObject> {
    match serde_json::from_str::<serde_json::Value>(json).context("config is not valid JSON")? {
        serde_json::Value::Object(object) => Ok(object),
        _ => anyhow::bail!("config must be a JSON object"),
    }
}

/// Parse a stored integration config, decrypting its secret fields.
pub fn open_integration_config(kind: &str, stored: &str) -> Result<IntegrationConfig> {
    let mut object = parse_object(stored)?;
    secret_key::open_json_fields(
        &mut object,
        INTEGRATION_SECRET_FIELDS,
        Purpose::IntegrationSecret,
    )?;
    let json = serde_json::Value::Object(object).to_string();
    IntegrationConfig::parse(kind, &json)
}

/// Validate a requested integration config and return its stored form.
///
/// Secret fields are write-only: one that is missing or empty in `requested`
/// keeps the value saved in `existing` (the stored config being updated);
/// an explicit `null` clears it (for optional secrets such as an ntfy token).
/// The merged config must parse as `kind`; secrets are then encrypted.
pub fn seal_integration_config(
    kind: &str,
    requested: &serde_json::Value,
    existing: Option<&str>,
) -> Result<String> {
    let mut object = match requested {
        serde_json::Value::Object(object) => object.clone(),
        _ => anyhow::bail!("config must be a JSON object"),
    };
    let saved = existing.map(parse_object).transpose()?.unwrap_or_default();
    for field in INTEGRATION_SECRET_FIELDS {
        let provided = object
            .get(*field)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if provided {
            continue;
        }
        let cleared = object.get(*field).is_some_and(serde_json::Value::is_null);
        object.remove(*field);
        if cleared {
            continue;
        }
        if let Some(value) = saved.get(*field) {
            // Still encrypted: decrypt it for validation, re-sealed below.
            let mut one = JsonObject::new();
            one.insert((*field).to_string(), value.clone());
            secret_key::open_json_fields(&mut one, &[field], Purpose::IntegrationSecret)?;
            if let Some(plain) = one.remove(*field) {
                object.insert((*field).to_string(), plain);
            }
        }
    }

    let json = serde_json::Value::Object(object.clone()).to_string();
    IntegrationConfig::parse(kind, &json)?;

    secret_key::seal_json_fields(
        &mut object,
        INTEGRATION_SECRET_FIELDS,
        Purpose::IntegrationSecret,
    )?;
    Ok(serde_json::Value::Object(object).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_env_round_trips_and_is_not_stored_in_plaintext() {
        let mut app = App::new("demo".into(), "image".into(), None, None, 3000);
        let env = BTreeMap::from([("API_KEY".to_string(), "sk-live-123456".to_string())]);
        seal_new_app(&mut app, &env).expect("seal");

        let stored = app.env_vars.clone().unwrap_or_default();
        assert!(
            !stored.contains("sk-live-123456"),
            "env stored in plaintext"
        );
        assert!(
            !app.webhook_secret
                .as_deref()
                .unwrap_or_default()
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
            "webhook secret must be encrypted, not the raw hex key"
        );
        assert_eq!(app_env(&app).expect("open"), env);
        assert!(app_webhook_secret(&app).expect("open").is_some());
    }

    #[test]
    fn empty_env_is_stored_as_an_empty_object() {
        let mut app = App::new("demo".into(), "image".into(), None, None, 3000);
        seal_new_app(&mut app, &BTreeMap::new()).expect("seal");
        assert_eq!(app.env_vars.as_deref(), Some("{}"));
        assert!(app_env(&app).expect("open").is_empty());
    }

    #[test]
    fn database_secrets_round_trip() {
        let mut db = ManagedDatabase::new("pg".into(), "postgres".into(), None);
        db.custom_external_url = Some("postgres://u:pw@host/db".into());
        let plain_password = db.db_password.clone();
        let sealed = seal_database(&db).expect("seal");
        assert_ne!(sealed.db_password, plain_password);
        db.db_password = sealed.db_password;
        db.root_password = sealed.root_password;
        db.custom_external_url = sealed.custom_external_url;
        db.custom_internal_url = sealed.custom_internal_url;
        let opened = open_database(db).expect("open");
        assert_eq!(opened.db_password, plain_password);
        assert_eq!(
            opened.custom_external_url.as_deref(),
            Some("postgres://u:pw@host/db")
        );
    }

    #[test]
    fn integration_update_keeps_saved_secret_when_omitted() {
        let created = seal_integration_config(
            "telegram",
            &serde_json::json!({ "bot_token": "123:ABC-secret", "chat_id": "42" }),
            None,
        )
        .expect("create");
        assert!(
            !created.contains("123:ABC-secret"),
            "token stored in plaintext"
        );

        // Update without the token: the saved one is kept.
        let updated = seal_integration_config(
            "telegram",
            &serde_json::json!({ "bot_token": "", "chat_id": "43" }),
            Some(&created),
        )
        .expect("update");
        match open_integration_config("telegram", &updated).expect("open") {
            IntegrationConfig::Telegram(cfg) => {
                assert_eq!(cfg.bot_token, "123:ABC-secret");
                assert_eq!(cfg.chat_id, "43");
            }
            other => panic!("unexpected config {other:?}"),
        }

        // Replacing it works too.
        let replaced = seal_integration_config(
            "telegram",
            &serde_json::json!({ "bot_token": "999:NEW", "chat_id": "43" }),
            Some(&updated),
        )
        .expect("replace");
        match open_integration_config("telegram", &replaced).expect("open") {
            IntegrationConfig::Telegram(cfg) => assert_eq!(cfg.bot_token, "999:NEW"),
            other => panic!("unexpected config {other:?}"),
        }
    }

    #[test]
    fn an_optional_secret_is_cleared_by_null_and_kept_when_omitted() {
        let created = seal_integration_config(
            "ntfy",
            &serde_json::json!({ "topic": "alerts-x9", "token": "tk_1" }),
            None,
        )
        .expect("create");
        let kept =
            seal_integration_config("ntfy", &serde_json::json!({}), Some(&created)).expect("keep");
        match open_integration_config("ntfy", &kept).expect("open") {
            IntegrationConfig::Ntfy(cfg) => {
                assert_eq!(cfg.topic, "alerts-x9");
                assert_eq!(cfg.token.as_deref(), Some("tk_1"));
            }
            other => panic!("unexpected config {other:?}"),
        }
        let cleared =
            seal_integration_config("ntfy", &serde_json::json!({ "token": null }), Some(&kept))
                .expect("clear");
        match open_integration_config("ntfy", &cleared).expect("open") {
            IntegrationConfig::Ntfy(cfg) => {
                assert_eq!(cfg.topic, "alerts-x9", "other secrets are kept");
                assert_eq!(cfg.token, None);
            }
            other => panic!("unexpected config {other:?}"),
        }
        assert!(!cleared.contains("alerts-x9"), "topic stored in plaintext");
    }

    #[test]
    fn integration_create_without_secret_is_rejected() {
        assert!(
            seal_integration_config("slack", &serde_json::json!({}), None).is_err(),
            "a new Slack integration needs its webhook URL"
        );
    }
}
