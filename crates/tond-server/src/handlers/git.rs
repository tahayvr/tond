// handlers/git.rs — Git integration management endpoints
//
// Endpoints for managing per-app git integration settings:
//
//   PUT  /api/apps/{id}/auto-deploy
//     Enable or disable auto-deploy on webhook push. Also sets the branch
//     filter. Returns the updated app.
//
//   PUT  /api/apps/{id}/webhook-secret
//     Set or rotate the HMAC signing secret used to validate webhook payloads.
//     Accepts { "secret": "..." } or { "secret": null } to disable validation.
//
//   POST /api/apps/{id}/deploy-key
//     Generate (or regenerate) an Ed25519 SSH deploy key for the app.
//     Stores the private key on disk at $TOND_DATA_DIR/keys/<app_id>/id_ed25519
//     and the public key in the `deploy_key_pub` column. Returns the public key.
//
//   GET  /api/apps/{id}/deploy-key
//     Returns the current public deploy key (or 404 if none generated yet).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, AppResponse, DeployKeyResponse, TondError, UpdateAutoDeployRequest,
    UpdateSourceRequest, UpdateWebhookSecretRequest,
};

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    state::AppState,
};

/// Fetch app helper (same pattern as handlers.rs)
async fn fetch_app(db: &sqlx::SqlitePool, id_or_name: &str) -> Result<tond_common::App, AppError> {
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(id_or_name)
    .bind(id_or_name)
    .fetch_optional(db)
    .await?;

    app.ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.to_string())))
}

// ── Auto-deploy toggle ──────────────────────────────────────────────────────

/// PUT /api/apps/{id}/auto-deploy
///
/// Body: { "auto_deploy": true, "watch_branch": "main" }
/// Enables or disables automatic deploys when a webhook fires.
pub async fn update_auto_deploy(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<UpdateAutoDeployRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;
    let auto_deploy_val: i64 = if payload.auto_deploy { 1 } else { 0 };

    sqlx::query(
        "UPDATE apps
         SET auto_deploy = ?, watch_branch = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(auto_deploy_val)
    .bind(&payload.watch_branch)
    .bind(&app.id)
    .execute(&state.db)
    .await?;

    let updated = fetch_app(&state.db, &app.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Webhook secret ──────────────────────────────────────────────────────────

/// PUT /api/apps/{id}/webhook-secret
///
/// Body: { "secret": "my-secret" } or { "secret": null }
/// Sets the HMAC secret used to validate GitHub/GitLab webhook signatures.
pub async fn update_webhook_secret(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<UpdateWebhookSecretRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;

    // Normalize empty string to NULL so that webhook handlers skip signature
    // validation when no secret is set (consistent with null/omitted secret).
    let secret: Option<String> = payload.secret.filter(|s| !s.is_empty());

    sqlx::query("UPDATE apps SET webhook_secret = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&secret)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    let updated = fetch_app(&state.db, &app.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Deploy keys ─────────────────────────────────────────────────────────────

/// POST /api/apps/{id}/deploy-key
///
/// Generate (or regenerate) an Ed25519 SSH deploy key for this app.
/// Private key stored at $TOND_DATA_DIR/keys/<app_id>/id_ed25519 (mode 0600).
/// Public key stored in the apps.deploy_key_pub column.
pub async fn generate_deploy_key(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;

    let (public_key_str, private_key_pem) = generate_ed25519_keypair(&app.name)
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    // Persist private key to disk
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let key_dir = format!("{}/keys/{}", data_dir, app.id);
    tokio::fs::create_dir_all(&key_dir)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let key_path = format!("{}/id_ed25519", key_dir);
    tokio::fs::write(&key_path, &private_key_pem)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    // Set file permissions to 0600 (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&key_path, perms)
            .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    }

    // Store public key in DB
    sqlx::query("UPDATE apps SET deploy_key_pub = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&public_key_str)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    tracing::info!("Generated deploy key for app {}", app.name);

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(DeployKeyResponse {
            app_id: app.id,
            public_key: public_key_str,
        })),
    ))
}

/// GET /api/apps/{id}/deploy-key
///
/// Returns the current public deploy key. 404 if no key has been generated yet.
pub async fn get_deploy_key(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;

    let public_key = app.deploy_key_pub.ok_or_else(|| {
        AppError(TondError::AppNotFound(format!(
            "no deploy key for {}",
            app_id
        )))
    })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(DeployKeyResponse {
            app_id: app.id,
            public_key,
        })),
    ))
}

// ── SSH key generation ──────────────────────────────────────────────────────

/// Generate a fresh Ed25519 SSH keypair.
/// Returns (public_key_openssh, private_key_openssh_pem).
fn generate_ed25519_keypair(comment: &str) -> anyhow::Result<(String, String)> {
    use ssh_key::{Algorithm, PrivateKey, rand_core::OsRng};

    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)?;

    // Encode private key as OpenSSH PEM
    let private_pem = private_key.to_openssh(ssh_key::LineEnding::LF)?;

    // Encode public key in OpenSSH authorized_keys format with comment
    let public_key = private_key.public_key();
    let public_str = format!("{} {}", public_key.to_openssh()?, comment);

    Ok((public_str, private_pem.to_string()))
}

// ── Source update ───────────────────────────────────────────────────────────

/// PUT /api/apps/{id}/source
///
/// Body: { "source_type": "git"|"image"|"tarball", "source_url": "..." }
/// Updates the deployment source type and URL for the app.
pub async fn update_source(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<UpdateSourceRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate source_type
    match payload.source_type.as_str() {
        "git" | "image" | "tarball" | "dockerfile" | "compose" => {}
        other => {
            return Err(AppError(TondError::InvalidInput(format!(
                "invalid source_type '{}': must be 'git', 'image', 'tarball', 'dockerfile', or 'compose'",
                other
            ))));
        }
    }

    let app = fetch_app(&state.db, &app_id).await?;

    sqlx::query(
        "UPDATE apps SET source_type = ?, source_url = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&payload.source_type)
    .bind(&payload.source_url)
    .bind(&app.id)
    .execute(&state.db)
    .await?;

    let updated = fetch_app(&state.db, &app.id).await?;
    tracing::info!(
        "Updated source for app {} to type={} url={:?}",
        app.name,
        payload.source_type,
        payload.source_url
    );
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}
