// handlers/storage_providers.rs — Storage provider CRUD + connectivity test
//
// Routes:
//   GET    /api/storage-providers          — list all configured providers
//   GET    /api/storage-providers/:id      — get single provider
//   POST   /api/storage-providers          — create provider (secret encrypted at rest)
//   PATCH  /api/storage-providers/:id      — update name or config (partial)
//   DELETE /api/storage-providers/:id      — delete (fails if referenced by backup_config)
//   POST   /api/storage-providers/:id/test — verify connectivity (ListObjects)
//
// The `secret_access_key` in the S3 config is AES-GCM encrypted using the same
// `secret_key::encrypt` / `secret_key::decrypt` helpers as the other secret columns.
// The encrypted value is stored as `secret_access_key_enc`; the plaintext key is
// never persisted and is stripped from all API responses.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tond_common::*;
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAdmin, RequireAuth},
    error::AppError,
    secret_key::{self, Purpose},
    state::AppState,
    storage,
};

// ── List ──────────────────────────────────────────────────────────────────────

pub async fn list_storage_providers(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, StorageProvider>(
        "SELECT * FROM storage_providers ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;

    let resp: Vec<StorageProviderResponse> = rows.into_iter().map(to_response).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}

// ── Get single ────────────────────────────────────────────────────────────────

pub async fn get_storage_provider(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_provider(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(to_response(row)))))
}

// ── Create ────────────────────────────────────────────────────────────────────

pub async fn create_storage_provider(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(req): Json<CreateStorageProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.name.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name is required".to_string(),
        )));
    }
    if req.kind != "s3" {
        return Err(AppError(TondError::InvalidInput(
            "kind must be 's3'".to_string(),
        )));
    }

    let config_str = encrypt_config(&req.kind, &req.config)?;
    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO storage_providers (id, name, kind, config)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(&req.kind)
    .bind(&config_str)
    .execute(&state.db)
    .await?;

    let row = fetch_provider(&state.db, &id).await?;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(to_response(row)))))
}

// ── Update (PATCH — partial) ──────────────────────────────────────────────────

pub async fn update_storage_provider(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateStorageProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_provider(&state.db, &id).await?;

    let name = req
        .name
        .map(|n| n.trim().to_string())
        .unwrap_or(row.name.clone());
    if name.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name cannot be empty".to_string(),
        )));
    }

    let config_str = match req.config {
        Some(ref cfg) => encrypt_config(&row.kind, cfg)?,
        None => row.config,
    };

    sqlx::query(
        "UPDATE storage_providers
         SET name = ?, config = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&name)
    .bind(&config_str)
    .bind(&id)
    .execute(&state.db)
    .await?;

    let updated = fetch_provider(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(to_response(updated)))))
}

// ── Delete ────────────────────────────────────────────────────────────────────

pub async fn delete_storage_provider(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Guard: refuse if this provider is referenced by backup_config
    let in_use: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM backup_config WHERE storage_provider_id = ?)",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    if in_use {
        return Err(AppError(TondError::InvalidInput(
            "This provider is used by backup settings. Remove it from backup settings first."
                .to_string(),
        )));
    }

    let deleted = sqlx::query("DELETE FROM storage_providers WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();

    if deleted == 0 {
        return Err(AppError(TondError::NotFound(format!(
            "Storage provider {id} not found"
        ))));
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "deleted": id }))),
    ))
}

// ── Test connectivity ─────────────────────────────────────────────────────────

pub async fn test_storage_provider(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_provider(&state.db, &id).await?;

    // Decrypt the secret before testing
    let provider_with_secret = decrypt_provider_secret(row).map_err(|e| {
        AppError(TondError::Internal(format!(
            "Failed to decrypt secret: {e}"
        )))
    })?;

    storage::test_connectivity(&provider_with_secret)
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "Connectivity test failed: {e}"
            )))
        })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "ok": true, "message": "Connected successfully" }),
        )),
    ))
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub async fn fetch_provider(db: &sqlx::SqlitePool, id: &str) -> Result<StorageProvider, AppError> {
    sqlx::query_as::<_, StorageProvider>("SELECT * FROM storage_providers WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| {
            AppError(TondError::NotFound(format!(
                "Storage provider {id} not found"
            )))
        })
}

/// Validate the incoming config, encrypt the `secret_access_key`, and return
/// the canonical JSON string to store in the DB.
fn encrypt_config(kind: &str, config: &serde_json::Value) -> Result<String, AppError> {
    if kind != "s3" {
        return Err(AppError(TondError::InvalidInput(
            "only 's3' kind is supported".to_string(),
        )));
    }

    let mut cfg: serde_json::Map<String, serde_json::Value> =
        config.as_object().cloned().ok_or_else(|| {
            AppError(TondError::InvalidInput(
                "config must be a JSON object".to_string(),
            ))
        })?;

    // If the caller supplied a plaintext `secret_access_key`, encrypt it.
    if let Some(plaintext) = cfg.remove("secret_access_key") {
        let plain_str = plaintext.as_str().ok_or_else(|| {
            AppError(TondError::InvalidInput(
                "secret_access_key must be a string".to_string(),
            ))
        })?;
        let encrypted = secret_key::encrypt(Purpose::StorageSecretKey, plain_str)
            .map_err(|e| AppError(TondError::Internal(format!("Encryption failed: {e}"))))?;
        cfg.insert("secret_access_key_enc".to_string(), json!(encrypted));
    }

    // Validate required fields
    for field in &["region", "bucket", "access_key_id"] {
        if !cfg.contains_key(*field) || cfg[*field].as_str().unwrap_or("").is_empty() {
            return Err(AppError(TondError::InvalidInput(format!(
                "config.{field} is required"
            ))));
        }
    }

    // `secret_access_key_enc` must be present after the operation above
    if !cfg.contains_key("secret_access_key_enc") {
        return Err(AppError(TondError::InvalidInput(
            "config.secret_access_key is required".to_string(),
        )));
    }

    serde_json::to_string(&cfg)
        .map_err(|e| AppError(TondError::Internal(format!("JSON serialise failed: {e}"))))
}

/// Build the outbound response, stripping `secret_access_key_enc` from the
/// returned config object.
fn to_response(row: StorageProvider) -> StorageProviderResponse {
    let mut config: serde_json::Value =
        serde_json::from_str(&row.config).unwrap_or(serde_json::Value::Null);

    // Remove the encrypted secret key from the response
    if let Some(obj) = config.as_object_mut() {
        obj.remove("secret_access_key_enc");
    }

    StorageProviderResponse {
        id: row.id,
        name: row.name,
        kind: row.kind,
        config,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Returns a clone of the StorageProvider with the `secret_access_key_enc` in
/// the config JSON decrypted back to `secret_access_key` (plaintext), suitable
/// for passing into the storage module.
pub fn decrypt_provider_secret(row: StorageProvider) -> anyhow::Result<StorageProvider> {
    let mut cfg: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&row.config)?;

    if let Some(enc_val) = cfg.remove("secret_access_key_enc") {
        let enc_str = enc_val
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("secret_access_key_enc is not a string"))?;
        let plaintext = secret_key::decrypt(Purpose::StorageSecretKey, enc_str)?;
        cfg.insert(
            "secret_access_key".to_string(),
            serde_json::Value::String(plaintext),
        );
    }

    Ok(StorageProvider {
        config: serde_json::to_string(&cfg)?,
        ..row
    })
}

#[cfg(test)]
mod tests {
    use tond_common::StorageProvider;

    use super::*;

    fn sample_provider(config: &str) -> StorageProvider {
        StorageProvider {
            id: "provider-1".to_string(),
            name: "Backups".to_string(),
            kind: "s3".to_string(),
            config: config.to_string(),
            created_at: "2026-04-08T12:00:00Z".to_string(),
            updated_at: "2026-04-08T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn encrypt_config_validates_required_fields_and_encrypts_secret_access_key() {
        let config = serde_json::json!({
            "region": "us-east-1",
            "bucket": "tond-backups",
            "access_key_id": "AKIA123",
            "secret_access_key": "secret-value",
            "endpoint": "https://s3.example.com"
        });

        let encrypted = encrypt_config("s3", &config).unwrap_or_else(|err| {
            panic!("encrypt_config should accept valid S3 config: {}", err.0)
        });
        let encrypted_json: serde_json::Value = serde_json::from_str(&encrypted)
            .unwrap_or_else(|err| panic!("encrypted config should remain valid JSON: {err}"));

        assert!(
            encrypted_json.get("secret_access_key").is_none(),
            "encrypt_config should never persist the plaintext secret access key"
        );
        assert!(
            encrypted_json
                .get("secret_access_key_enc")
                .and_then(|v| v.as_str())
                .is_some(),
            "encrypt_config should replace the plaintext secret with an encrypted secret_access_key_enc field"
        );
    }

    #[test]
    fn encrypt_config_rejects_invalid_kind_shape_and_missing_fields() {
        assert!(
            encrypt_config("gcs", &serde_json::json!({})).is_err(),
            "encrypt_config should reject unsupported provider kinds"
        );
        assert!(
            encrypt_config("s3", &serde_json::json!("not-an-object"))
                .err()
                .unwrap_or_else(|| panic!("expected non-object config to be rejected"))
                .0
                .to_string()
                .contains("config must be a JSON object"),
            "encrypt_config should reject non-object config payloads"
        );
        assert!(
            encrypt_config(
                "s3",
                &serde_json::json!({
                    "region": "us-east-1",
                    "access_key_id": "AKIA123",
                    "secret_access_key": "secret-value"
                }),
            )
            .err()
            .unwrap_or_else(|| panic!("expected missing bucket to be rejected"))
            .0
            .to_string()
            .contains("config.bucket is required"),
            "encrypt_config should require all mandatory S3 fields"
        );
    }

    #[test]
    fn to_response_strips_encrypted_secret_from_outbound_config() {
        let row = sample_provider(
            r#"{"region":"us-east-1","bucket":"tond-backups","access_key_id":"AKIA123","secret_access_key_enc":"ciphertext"}"#,
        );

        let response = to_response(row);
        assert_eq!(
            response.config,
            serde_json::json!({
                "region": "us-east-1",
                "bucket": "tond-backups",
                "access_key_id": "AKIA123"
            }),
            "to_response should omit secret_access_key_enc from API responses"
        );
    }

    #[test]
    fn decrypt_provider_secret_restores_plaintext_secret_access_key() {
        let encrypted = secret_key::encrypt(Purpose::StorageSecretKey, "secret-value")
            .unwrap_or_else(|err| panic!("password encryption should succeed: {err}"));
        let row = sample_provider(&format!(
            r#"{{"region":"us-east-1","bucket":"tond-backups","access_key_id":"AKIA123","secret_access_key_enc":"{}"}}"#,
            encrypted
        ));

        let decrypted = decrypt_provider_secret(row)
            .unwrap_or_else(|err| panic!("decrypt_provider_secret should succeed: {err}"));
        let config: serde_json::Value = serde_json::from_str(&decrypted.config)
            .unwrap_or_else(|err| panic!("decrypted provider config should be valid JSON: {err}"));

        assert_eq!(
            config,
            serde_json::json!({
                "region": "us-east-1",
                "bucket": "tond-backups",
                "access_key_id": "AKIA123",
                "secret_access_key": "secret-value"
            }),
            "decrypt_provider_secret should replace the encrypted secret with the plaintext secret_access_key for storage operations"
        );
    }
}
