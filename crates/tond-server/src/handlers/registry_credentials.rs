// handlers/registry_credentials.rs — Registry credential management endpoints
//
// Endpoints:
//
//   GET    /api/registry-credentials
//     List all stored registry credentials. Passwords are never returned.
//
//   POST   /api/registry-credentials
//     Create a new credential. The password is encrypted with AES-256-GCM
//     before storage. The key is derived from the TOND_SECRET_KEY env var
//     (falls back to a fixed dev key if unset — warn loudly).
//
//   DELETE /api/registry-credentials/{id}
//     Delete a credential. Also detaches it from any apps that reference it
//     (handled automatically by the ON DELETE SET NULL FK constraint).
//
//   PUT    /api/apps/{id}/registry-credential
//     Attach or detach a registry credential from an app. Pass
//     { "registry_credential_id": "<id>" } to attach, or
//     { "registry_credential_id": null } to detach.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Nonce},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use sha2::{Digest, Sha256};
use tond_common::{
    ApiResponse, AppResponse, CreateRegistryCredentialRequest, RegistryCredential,
    RegistryCredentialResponse, SetRegistryCredentialRequest, TondError,
};
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    state::AppState,
};

// ── Key derivation ──────────────────────────────────────────────────────────

/// Derive a 32-byte AES key from TOND_SECRET_KEY using SHA-256.
/// Falls back to a fixed dev key with a loud warning.
fn derive_key() -> [u8; 32] {
    let secret = std::env::var("TOND_SECRET_KEY").unwrap_or_else(|_| {
        tracing::warn!(
            "TOND_SECRET_KEY not set — registry credential passwords are using the \
             insecure dev key. Set TOND_SECRET_KEY in production!"
        );
        "tond-dev-insecure-key-do-not-use-in-prod".to_string()
    });
    let hash = Sha256::digest(secret.as_bytes());
    hash.into()
}

/// Encrypt a plaintext password. Returns base64(nonce || ciphertext).
pub fn encrypt_password(plaintext: &str) -> anyhow::Result<String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {}", e))?;

    let mut nonce_bytes = [0u8; 12];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut nonce_bytes);
    let nonce = Nonce::<Aes256Gcm>::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("encryption failed: {}", e))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(B64.encode(&combined))
}

/// Decrypt a stored password from base64(nonce || ciphertext).
pub fn decrypt_password(encrypted: &str) -> anyhow::Result<String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {}", e))?;

    let combined = B64
        .decode(encrypted)
        .map_err(|e| anyhow::anyhow!("base64 decode failed: {}", e))?;

    if combined.len() < 12 {
        anyhow::bail!("encrypted payload too short");
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes)
        .map_err(|e| anyhow::anyhow!("invalid nonce: {}", e))?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {}", e))?;

    String::from_utf8(plaintext).map_err(|e| anyhow::anyhow!("utf8 decode failed: {}", e))
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/registry-credentials
pub async fn list_registry_credentials(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let creds =
        sqlx::query_as::<_, RegistryCredential>("SELECT * FROM registry_credentials ORDER BY name")
            .fetch_all(&state.db)
            .await?;

    let responses: Vec<RegistryCredentialResponse> = creds.into_iter().map(Into::into).collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/registry-credentials
pub async fn create_registry_credential(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Json(payload): Json<CreateRegistryCredentialRequest>,
) -> Result<impl IntoResponse, AppError> {
    if payload.name.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name is required".to_string(),
        )));
    }
    if payload.server.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "server is required".to_string(),
        )));
    }
    if payload.password.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "password is required".to_string(),
        )));
    }

    let password_enc = encrypt_password(&payload.password)
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO registry_credentials (id, name, server, username, password_enc)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(payload.name.trim())
    .bind(payload.server.trim())
    .bind(payload.username.trim())
    .bind(&password_enc)
    .execute(&state.db)
    .await?;

    let cred =
        sqlx::query_as::<_, RegistryCredential>("SELECT * FROM registry_credentials WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(RegistryCredentialResponse::from(cred))),
    ))
}

/// DELETE /api/registry-credentials/{id}
pub async fn delete_registry_credential(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(cred_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rows_affected = sqlx::query("DELETE FROM registry_credentials WHERE id = ?")
        .bind(&cred_id)
        .execute(&state.db)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(AppError(TondError::NotFound(format!(
            "Registry credential {} not found",
            cred_id
        ))));
    }

    Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))))
}

/// PUT /api/apps/{id}/registry-credential
///
/// Attach or detach a registry credential from an app.
pub async fn set_app_registry_credential(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<SetRegistryCredentialRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify app exists
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&app_id)
    .bind(&app_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::AppNotFound(app_id.clone())))?;

    // If attaching, verify the credential exists
    if let Some(ref cred_id) = payload.registry_credential_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registry_credentials WHERE id = ?)")
                .bind(cred_id)
                .fetch_one(&state.db)
                .await?;

        if !exists {
            return Err(AppError(TondError::NotFound(format!(
                "Registry credential {} not found",
                cred_id
            ))));
        }
    }

    sqlx::query(
        "UPDATE apps SET registry_credential_id = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&payload.registry_credential_id)
    .bind(&app.id)
    .execute(&state.db)
    .await?;

    let updated = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps WHERE id = ? LIMIT 1")
        .bind(&app.id)
        .fetch_one(&state.db)
        .await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}
