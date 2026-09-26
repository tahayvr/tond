// handlers/git.rs — Git integration management endpoints
//
// Endpoints for managing per-app git integration settings:
//
//   PUT  /api/apps/{id}/auto-deploy
//     Enable or disable auto-deploy on webhook push. Also sets the branch
//     filter. Returns the updated app.
//
//   GET  /api/apps/{id}/webhook
//     Webhook URLs (per provider) and whether a secret is set, for setting up
//     GitHub/GitLab/Gitea/generic webhooks. The secret itself is write-only.
//
//   GET  /api/apps/{id}/webhook-secret
//     Reveal the webhook secret (admin only, audit-logged).
//
//   PUT  /api/apps/{id}/webhook-secret
//     Replace the webhook secret with a caller-chosen value ({ "secret": "..." },
//     at least MIN_WEBHOOK_SECRET_LEN characters). Verification cannot be
//     disabled: every webhook must be signed.
//
//   POST /api/apps/{id}/webhook-secret/rotate
//     Replace the webhook secret with a fresh random one; returns the new
//     secret once (audit-logged).
//
//   POST /api/apps/{id}/deploy-key
//     Generate (or regenerate) an Ed25519 SSH deploy key for the app.
//     Stores the private key on disk at $TOND_DATA_DIR/keys/<app_id>/id_ed25519
//     and the public key in the `deploy_key_pub` column. Returns the public key.
//
//   GET  /api/apps/{id}/deploy-key
//     Returns the current public deploy key (or 404 if none generated yet).
//
//   POST /api/git/branches
//     List a repository's branches and default branch (`git ls-remote`) for
//     the new-app wizard's branch picker.

use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tond_common::{
    ApiResponse, AppResponse, DeployKeyResponse, GitHubApp, GitLabToken, MIN_WEBHOOK_SECRET_LEN,
    Scope, TondError, UpdateAutoDeployRequest, UpdateSourceRequest, UpdateWebhookSecretRequest,
    WebhookConfigResponse, WebhookSecretResponse, WebhookUrls, generate_raw_key,
};

use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    deploy::git::{
        GIT_ALLOW_PROTOCOL, GitHttpAuth, git_auth_env, ls_remote_args, parse_ls_remote,
        redact_url_credentials, split_url_credentials,
    },
    error::AppError,
    handlers::{github_apps, gitlab_tokens},
    secrets,
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
    auth: Authz<scope::AppsWrite>,
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
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("auto_deploy_update")
            .resource("app", &updated.id, &updated.name)
            .detail(format!("auto_deploy={}", payload.auto_deploy)),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Webhook secret ──────────────────────────────────────────────────────────

/// Build the webhook config for an app. Absolute URLs use the dashboard domain
/// (the proxy routes `/api` there); without one only paths are returned.
async fn webhook_config(
    state: &AppState,
    app: tond_common::App,
) -> Result<WebhookConfigResponse, AppError> {
    let has_secret = app.webhook_secret.as_deref().is_some_and(|s| !s.is_empty());
    let domain = state.settings.read().await.dashboard_domain.clone();
    let paths = WebhookUrls::for_app(&app.id);
    let urls = (!domain.is_empty()).then(|| {
        let base = format!("https://{domain}");
        WebhookUrls {
            github: format!("{base}{}", paths.github),
            gitlab: format!("{base}{}", paths.gitlab),
            gitea: format!("{base}{}", paths.gitea),
            generic: format!("{base}{}", paths.generic),
        }
    });
    Ok(WebhookConfigResponse {
        app_id: app.id,
        paths,
        urls,
        has_secret,
        auto_deploy: app.auto_deploy != 0,
        watch_branch: app.watch_branch,
    })
}

/// GET /api/apps/{id}/webhook
///
/// Returns the webhook URLs for every provider and whether a secret is set.
pub async fn get_webhook_config(
    _rate: RateLimit,
    _auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;
    let config = webhook_config(&state, app).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(config))))
}

/// GET /api/apps/{id}/webhook-secret
///
/// Reveals the current secret, for pasting into a provider's webhook form.
/// Needs env:read-values (a stored secret, like env values); every reveal is
/// audit-logged.
pub async fn reveal_webhook_secret(
    _rate: RateLimit,
    auth: Authz<scope::EnvReadValues>,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;
    let secret = secrets::app_webhook_secret(&app)?.ok_or_else(|| {
        AppError(TondError::NotFound(format!(
            "webhook secret of app '{}'",
            app.name
        )))
    })?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("webhook_secret_reveal").resource("app", &app.id, &app.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(WebhookSecretResponse { secret })),
    ))
}

/// PUT /api/apps/{id}/webhook-secret
///
/// Body: { "secret": "my-long-secret" }
/// Replaces the secret used to verify GitHub/Gitea/generic signatures and the
/// GitLab token. There is no way to clear it.
pub async fn update_webhook_secret(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<UpdateWebhookSecretRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;

    let secret = payload.secret.trim().to_string();
    validate_webhook_secret(&secret)?;

    sqlx::query("UPDATE apps SET webhook_secret = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(secrets::seal_webhook_secret(&secret)?)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    let updated = fetch_app(&state.db, &app.id).await?;
    let config = webhook_config(&state, updated).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("webhook_secret_update").resource("app", &app.id, &app.name),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(config))))
}

/// Reject secrets that are too short to resist guessing or that contain
/// characters a provider's settings form would mangle.
fn validate_webhook_secret(secret: &str) -> Result<(), AppError> {
    if secret.chars().count() < MIN_WEBHOOK_SECRET_LEN {
        return Err(AppError(TondError::InvalidInput(format!(
            "webhook secret must be at least {MIN_WEBHOOK_SECRET_LEN} characters \
             (rotate it to have one generated)"
        ))));
    }
    if secret.chars().any(char::is_control) {
        return Err(AppError(TondError::InvalidInput(
            "webhook secret must not contain control characters".to_string(),
        )));
    }
    Ok(())
}

/// POST /api/apps/{id}/webhook-secret/rotate
///
/// Generates a new random secret. The old one stops working immediately, so
/// the provider's webhook settings must be updated with the returned value,
/// which is shown only in this response.
pub async fn rotate_webhook_secret(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &app_id).await?;
    let secret = generate_raw_key();

    sqlx::query("UPDATE apps SET webhook_secret = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(secrets::seal_webhook_secret(&secret)?)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    tracing::info!("Rotated webhook secret for app {}", app.name);
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("webhook_secret_rotate").resource("app", &app.id, &app.name),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(WebhookSecretResponse { secret })),
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
    auth: Authz<scope::AppsWrite>,
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

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("deploy_key_generate").resource("app", &app.id, &app.name),
    )
    .await;
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
    _auth: Authz<scope::AppsRead>,
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
    auth: Authz<scope::AppsWrite>,
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

    // Compose files are full container specs; switching to or from one is an admin action.
    if payload.source_type == "compose" || app.source_type == "compose" {
        auth.require_to(Scope::SettingsWrite, "change a Docker Compose app's source")?;
    }

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
        payload.source_url.as_deref().map(redact_url_credentials)
    );
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("source_update")
            .resource("app", &app.id, &app.name)
            .detail(format!("source_type={}", payload.source_type)),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Remote branches (new-app wizard) ────────────────────────────────────────

/// POST /api/git/branches request body.
#[derive(Debug, Deserialize)]
pub struct RemoteBranchesRequest {
    /// Repository URL (http/https; credentials in the URL are used, not echoed)
    pub url: String,
    /// GitHub App whose installation token to use for private repos
    pub github_app_id: Option<String>,
    /// GitLab token to use for private repos
    pub gitlab_token_id: Option<String>,
}

/// POST /api/git/branches response.
#[derive(Debug, Serialize)]
pub struct RemoteBranchesResponse {
    /// The repository's default branch (its HEAD), when the server says
    pub default_branch: Option<String>,
    /// Branch names, default first (at most MAX_BRANCHES)
    pub branches: Vec<String>,
    /// More branches exist than were returned
    pub truncated: bool,
}

/// Branches returned to the picker.
const MAX_BRANCHES: usize = 200;
/// `git ls-remote` gives up after this long.
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(15);

/// POST /api/git/branches
///
/// Body: { "url": "https://github.com/o/r.git", "github_app_id"?: "...", "gitlab_token_id"?: "..." }
/// Lists a repository's branches and its default branch with
/// `git ls-remote`, so the new-app wizard can offer a branch picker instead
/// of assuming `main`. Only http(s) URLs: SSH needs a deploy key, which an
/// app gets after it is created.
pub async fn list_remote_branches(
    _rate: RateLimit,
    _auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Json(req): Json<RemoteBranchesRequest>,
) -> Result<impl IntoResponse, AppError> {
    let raw = req.url.trim();
    if !(raw.starts_with("https://") || raw.starts_with("http://")) {
        return Err(AppError(TondError::InvalidInput(
            "Branches can only be listed for http(s) Git URLs; type the branch name instead"
                .to_string(),
        )));
    }
    let (url, mut auth) = split_url_credentials(raw);

    if let Some(id) = req.github_app_id.as_deref()
        && let Some(gh_app) =
            sqlx::query_as::<_, GitHubApp>("SELECT * FROM github_apps WHERE id = ? LIMIT 1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?
    {
        let token = github_apps::generate_installation_token(&gh_app)
            .await
            .map_err(|e| AppError(TondError::InvalidInput(format!("GitHub App token: {e}"))))?;
        auth = Some(GitHttpAuth {
            username: "x-access-token".to_string(),
            password: token,
        });
    } else if let Some(id) = req.gitlab_token_id.as_deref()
        && let Some(gl) =
            sqlx::query_as::<_, GitLabToken>("SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?
    {
        auth = Some(
            gitlab_tokens::gitlab_git_credentials(&gl)
                .map_err(|e| AppError(TondError::InvalidInput(e.to_string())))?,
        );
    }

    let mut cmd = crate::process::command("git");
    cmd.args(ls_remote_args(&url))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", GIT_ALLOW_PROTOCOL)
        .kill_on_drop(true);
    if let Some(auth) = &auth {
        cmd.envs(git_auth_env(&url, auth));
    }
    let output = tokio::time::timeout(LS_REMOTE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            AppError(TondError::InvalidInput(
                "The Git host didn't answer in time; type the branch name instead".to_string(),
            ))
        })?
        .map_err(|e| AppError(TondError::Internal(format!("could not run git: {e}"))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError(TondError::InvalidInput(ls_remote_error(&stderr))));
    }

    let mut parsed = parse_ls_remote(&String::from_utf8_lossy(&output.stdout));
    let truncated = parsed.branches.len() > MAX_BRANCHES;
    parsed.branches.truncate(MAX_BRANCHES);
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(RemoteBranchesResponse {
            default_branch: parsed.default_branch,
            branches: parsed.branches,
            truncated,
        })),
    ))
}

/// A readable message for a failed `git ls-remote`.
fn ls_remote_error(stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("could not read username")
        || lower.contains("authentication failed")
        || lower.contains("not found")
        || lower.contains("403")
    {
        "Tond couldn't read this repository. If it's private, pick a connected GitHub App or add credentials; you can also type the branch name.".to_string()
    } else {
        format!(
            "Couldn't list branches: {}",
            redact_url_credentials(stderr.trim())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_errors_are_readable_and_redacted() {
        assert!(
            ls_remote_error("fatal: could not read Username for 'https://github.com'")
                .contains("private")
        );
        let msg = ls_remote_error("fatal: unable to access 'https://u:secret@host/r.git/'");
        assert!(msg.starts_with("Couldn't list branches"));
        assert!(!msg.contains("secret"));
    }
}
