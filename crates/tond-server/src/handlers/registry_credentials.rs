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
//     (see secret_key.rs).
//
//   DELETE /api/registry-credentials/{id}
//     Delete a credential. Also detaches it from any apps that reference it
//     (handled automatically by the ON DELETE SET NULL FK constraint).
//
//   PUT    /api/apps/{id}/registry-credential
//     Attach or detach a registry credential from an app. Pass
//     { "registry_credential_id": "<id>" } to attach, or
//     { "registry_credential_id": null } to detach.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, AppResponse, CreateRegistryCredentialRequest, RegistryCredential,
    RegistryCredentialResponse, SetRegistryCredentialRequest, TondError,
};
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    secret_key::{self, SecretKeys},
    state::AppState,
};

// ── Encryption ──────────────────────────────────────────────────────────────

/// Encrypt a plaintext password with the current TOND_SECRET_KEY.
/// Returns base64(nonce || ciphertext).
pub fn encrypt_password(plaintext: &str) -> anyhow::Result<String> {
    secret_key::encrypt(plaintext, &SecretKeys::current().aes)
}

/// Decrypt a stored password from base64(nonce || ciphertext).
pub fn decrypt_password(encrypted: &str) -> anyhow::Result<String> {
    secret_key::decrypt(encrypted, &SecretKeys::current().aes)
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
