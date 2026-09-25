// handlers/github_apps.rs — GitHub App integration (Phase 4.4)
//
// Implements the full GitHub App Manifest flow:
//
//   GET  /api/github-apps
//     List all registered GitHub App registrations.
//
//   POST /api/github-apps/manifest
//     Generate and return a GitHub App manifest URL. The dashboard redirects
//     the user's browser to `https://github.com/settings/apps/new?manifest=<encoded>`.
//     Requires the server's `dashboard_domain` setting to be configured.
//     A stub row with a `manifest_state` UUID is inserted before the redirect so
//     the two subsequent GitHub callbacks can be correlated to this exact row.
//
//   GET  /api/github-apps/callback?code=<code>&state=<manifest_state>
//     GitHub redirects here after App creation. Exchanges the one-time code at
//     `POST https://api.github.com/app-manifests/{code}/conversions`, updates
//     the stub row identified by `state` with all returned credentials, then
//     redirects the browser to the dashboard integrations page.
//
//   GET  /webhooks/source/github/install?installation_id=<id>&setup_action=<action>&state=<manifest_state>
//     GitHub redirects here after the user installs the App on their repos.
//     Uses `state` to find the exact github_apps row and stores the
//     `installation_id` on it. Falls back to the most-recently-created row if
//     `state` is absent (backwards-compat / manual installs).
//
//   POST /api/github-apps/{id}/install
//     Returns the GitHub App installation URL so the dashboard can redirect the
//     user there. (Kept for completeness; the list endpoint also returns install_url.)
//
//   PATCH /api/github-apps/{id}
//     Update a registration (e.g. rename or set installation_id manually).
//
//   DELETE /api/github-apps/{id}
//     Remove a GitHub App registration. Detaches all apps that reference it.
//
//   GET  /api/github-apps/{id}/repos
//     Use the installation access token to list repositories the App is
//     installed on (`GET https://api.github.com/installation/repositories`).
//
//   GET  /api/github-apps/{id}/repos/{owner}/{repo}/branches
//     Return the branch list for a specific repo.
//
//   PUT  /api/apps/{app_id}/github-app
//     Link or unlink a GitHub App registration from an app.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Serialize;
use tond_common::{
    ApiResponse, GitHubApp, GitHubAppResponse, GitHubBranchResponse, GitHubCallbackQuery,
    GitHubInstallQuery, GitHubRepoResponse, SetGitHubAppRequest, TondError, UpdateGitHubAppRequest,
};

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    state::AppState,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn fetch_github_app(db: &sqlx::SqlitePool, id: &str) -> Result<GitHubApp, AppError> {
    let app = sqlx::query_as::<_, GitHubApp>("SELECT * FROM github_apps WHERE id = ? LIMIT 1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    app.ok_or_else(|| AppError(TondError::Internal(format!("GitHub App {} not found", id))))
}

/// Generate a GitHub App installation access token.
///
/// Steps:
///   1. Sign a JWT with RS256 using the App's private key (valid 10 min).
///   2. POST to `https://api.github.com/app/installations/{installation_id}/access_tokens`.
///   3. Return the short-lived token string.
pub async fn generate_installation_token(app: &GitHubApp) -> anyhow::Result<String> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde_json::json;

    let installation_id = app.installation_id.ok_or_else(|| {
        anyhow::anyhow!(
            "GitHub App {} has no installation_id — user must install the App first",
            app.id
        )
    })?;

    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "iat": now - 60,   // issued 60 s ago (clock skew tolerance)
        "exp": now + 540,  // expires in 9 minutes (max 10 min)
        "iss": app.app_id,
    });

    let encoding_key = EncodingKey::from_rsa_pem(app.private_key.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to parse GitHub App private key: {}", e))?;

    let jwt = encode(&Header::new(Algorithm::RS256), &claims, &encoding_key)
        .map_err(|e| anyhow::anyhow!("Failed to sign GitHub App JWT: {}", e))?;

    // Exchange JWT for an installation access token
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "https://api.github.com/app/installations/{}/access_tokens",
            installation_id
        ))
        .header("Authorization", format!("Bearer {}", jwt))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "tond-server")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GitHub API request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub API returned {}: {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;
    let token = json["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No token in GitHub API response"))?
        .to_string();

    Ok(token)
}

/// Post a commit status to the GitHub Statuses API.
///
/// `state` must be one of: "pending" | "success" | "failure" | "error".
/// `context` is the label shown in the PR checks list (e.g. "tond/preview").
/// `target_url` is the optional link that "Details" points to.
#[allow(clippy::too_many_arguments)]
pub async fn post_commit_status(
    gh_app: &GitHubApp,
    owner: &str,
    repo: &str,
    sha: &str,
    state: &str,
    description: &str,
    target_url: Option<&str>,
    context: &str,
) -> anyhow::Result<()> {
    let token = generate_installation_token(gh_app).await?;

    let mut body = serde_json::json!({
        "state": state,
        "description": description,
        "context": context,
    });

    if let Some(url) = target_url {
        body["target_url"] = serde_json::Value::String(url.to_string());
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "https://api.github.com/repos/{}/{}/statuses/{}",
            owner, repo, sha
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "tond-server")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GitHub Statuses API request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub Statuses API returned {}: {}", status, text);
    }

    tracing::debug!(
        "[commit-status] {}/{} {} → {} ({})",
        owner,
        repo,
        &sha[..sha.len().min(8)],
        state,
        context
    );

    Ok(())
}

/// Post or update a comment on a GitHub pull request.
///
/// If `existing_comment_id` is `Some`, issues a PATCH to update the existing
/// comment body. Otherwise creates a new comment with POST and returns the new
/// comment ID so it can be stored in `preview_deployments.comment_id`.
pub async fn post_or_update_pr_comment(
    gh_app: &GitHubApp,
    owner: &str,
    repo: &str,
    pr_number: i64,
    body_text: &str,
    existing_comment_id: Option<i64>,
) -> anyhow::Result<i64> {
    let token = generate_installation_token(gh_app).await?;
    let client = reqwest::Client::new();

    let payload = serde_json::json!({ "body": body_text });

    let resp = if let Some(comment_id) = existing_comment_id {
        // Update existing comment
        client
            .patch(format!(
                "https://api.github.com/repos/{}/{}/issues/comments/{}",
                owner, repo, comment_id
            ))
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "tond-server")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("GitHub Comments API PATCH failed: {}", e))?
    } else {
        // Create new comment
        client
            .post(format!(
                "https://api.github.com/repos/{}/{}/issues/{}/comments",
                owner, repo, pr_number
            ))
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "tond-server")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("GitHub Comments API POST failed: {}", e))?
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub Comments API returned {}: {}", status, text);
    }

    // Extract the comment ID from the response JSON
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse GitHub comment response: {}", e))?;
    let id = json["id"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("GitHub comment response missing 'id' field"))?;

    tracing::debug!(
        "[pr-comment] {}/{} PR#{} comment_id={}",
        owner,
        repo,
        pr_number,
        id
    );

    Ok(id)
}

/// Build a clone URL that injects an installation access token, e.g.:
/// `https://x-access-token:<token>@github.com/org/repo.git`
pub async fn build_clone_url_with_token(
    app: &GitHubApp,
    source_url: &str,
) -> anyhow::Result<String> {
    let token = generate_installation_token(app).await?;

    // Replace `https://github.com/` with `https://x-access-token:<token>@github.com/`
    let authenticated_url = if let Some(rest) = source_url.strip_prefix("https://github.com/") {
        format!("https://x-access-token:{}@github.com/{}", token, rest)
    } else {
        // Fall back: inject before the hostname
        source_url.replace("https://", &format!("https://x-access-token:{}@", token))
    };

    Ok(authenticated_url)
}

// ── GET /api/github-apps ──────────────────────────────────────────────────────

pub async fn list_github_apps(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    // Exclude pending stub rows (app_id = 0 means not yet confirmed by GitHub)
    let apps = sqlx::query_as::<_, GitHubApp>(
        "SELECT * FROM github_apps WHERE app_id != 0 ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;

    let responses: Vec<GitHubAppResponse> =
        apps.into_iter().map(GitHubAppResponse::from_app).collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── POST /api/github-apps/manifest ───────────────────────────────────────────

/// Returns a manifest URL that the dashboard uses to redirect the user to GitHub.
/// The manifest encodes all permissions, webhook URL, callback URL, etc.
///
/// We also insert a stub row with a fresh `manifest_state` UUID *before* the
/// redirect. This UUID is embedded in both `redirect_url` and `setup_url` so
/// GitHub echoes it back in each subsequent callback, letting us update the
/// correct row rather than always picking the most-recently-created one.
pub async fn get_manifest_url(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let domain = {
        let s = state.settings.read().await;
        s.dashboard_domain.clone()
    };

    if domain.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "dashboard_domain must be configured in Settings before connecting a GitHub App"
                .to_string(),
        )));
    }

    let base_url = format!("https://{}", domain);

    // Generate a stable state UUID for this registration attempt
    let manifest_state = uuid::Uuid::new_v4().to_string();
    let stub_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Insert a stub row so both callbacks can locate this exact registration
    sqlx::query(
        "INSERT INTO github_apps
         (id, name, app_id, client_id, client_secret, webhook_secret, private_key, slug,
          manifest_state, created_at, updated_at)
         VALUES (?, 'pending', 0, '', '', '', '', '', ?, ?, ?)",
    )
    .bind(&stub_id)
    .bind(&manifest_state)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // GitHub validates that redirect_url, callback_urls, setup_url, and
    // hook_attributes.url are absolute URLs with no query strings. The state
    // UUID is passed separately as a query param on the POST action URL
    // (github.com/settings/apps/new?state=<uuid>) and GitHub echoes it back
    // automatically in the final redirect — it must NOT be embedded inside
    // the manifest URLs themselves.
    let manifest = serde_json::json!({
        "name": format!("tond-{}", domain.replace('.', "-")),
        "url": base_url,
        "hook_attributes": {
            "url": format!("{}/webhooks/source/github/events", base_url),
            "active": true
        },
        "redirect_url": format!("{}/api/github-apps/callback", base_url),
        "callback_urls": [
            format!("{}/api/github-apps/callback", base_url)
        ],
        "setup_url": format!("{}/webhooks/source/github/install", base_url),
        "setup_on_update": true,
        "description": "Tond self-hosted PaaS — git deploy integration",
        "public": false,
        "default_permissions": {
            "contents": "read",
            "pull_requests": "write",
            "statuses": "write",
            "metadata": "read"
        },
        "default_events": ["push", "pull_request"]
    });

    // The dashboard must POST this manifest as a hidden form field to github_url.
    // GitHub's Manifest flow requires a POST submission — a GET redirect with the
    // manifest in the query string is silently ignored, landing the user on a blank
    // "Create New App" form instead of the pre-configured one.
    let github_url = format!(
        "https://github.com/settings/apps/new?state={}",
        manifest_state
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "github_url": github_url,
            "manifest": manifest.to_string()
        }))),
    ))
}

// ── GET /api/github-apps/callback?code=<code>&state=<manifest_state> ──────────

/// GitHub redirects here after App creation with a one-time code.
/// We exchange it for the full App credentials and store them.
/// Then we redirect the browser back to the dashboard integrations page.
pub async fn github_app_callback(
    State(state): State<AppState>,
    Query(params): Query<GitHubCallbackQuery>,
) -> Response {
    match exchange_manifest_code(&state, &params.code, params.state.as_deref()).await {
        Ok(_) => {
            // Redirect to the dashboard integrations page
            let domain = {
                let s = state.settings.read().await;
                s.dashboard_domain.clone()
            };
            let redirect = if domain.is_empty() {
                "/integrations/github".to_string()
            } else {
                format!("https://{}/integrations/github", domain)
            };
            Redirect::to(&redirect).into_response()
        }
        Err(e) => {
            tracing::error!("GitHub App manifest exchange failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiResponse::<()>::err(format!(
                    "GitHub App registration failed: {}",
                    e
                ))),
            )
                .into_response()
        }
    }
}

/// Exchange a one-time manifest code for App credentials.
///
/// If `manifest_state` is provided, we update the stub row that was created
/// during `get_manifest_url`. Otherwise we fall back to inserting a fresh row
/// (handles the case where the stub mechanism wasn't used, e.g. direct API calls).
async fn exchange_manifest_code(
    state: &AppState,
    code: &str,
    manifest_state: Option<&str>,
) -> anyhow::Result<GitHubAppResponse> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "https://api.github.com/app-manifests/{}/conversions",
            code
        ))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "tond-server")
        .header("Content-Length", "0")
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub manifest conversion failed ({}): {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;

    let app_id = json["id"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Missing id"))?;
    let client_id = json["client_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing client_id"))?
        .to_string();
    let client_secret = json["client_secret"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing client_secret"))?
        .to_string();
    let webhook_secret = json["webhook_secret"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing webhook_secret"))?
        .to_string();
    let pem = json["pem"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing pem"))?
        .to_string();
    let slug = json["slug"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing slug"))?
        .to_string();
    let name = json["name"].as_str().unwrap_or(&slug).to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Try to update the existing stub row identified by manifest_state
    let updated_id: Option<String> = if let Some(ms) = manifest_state {
        sqlx::query_scalar::<_, String>(
            "UPDATE github_apps
             SET name = ?, app_id = ?, client_id = ?, client_secret = ?,
                 webhook_secret = ?, private_key = ?, slug = ?, updated_at = ?
             WHERE manifest_state = ?
             RETURNING id",
        )
        .bind(&name)
        .bind(app_id)
        .bind(&client_id)
        .bind(&client_secret)
        .bind(&webhook_secret)
        .bind(&pem)
        .bind(&slug)
        .bind(&now)
        .bind(ms)
        .fetch_optional(&state.db)
        .await?
    } else {
        None
    };

    // If we couldn't update a stub row, insert a fresh one
    let final_id = if let Some(id) = updated_id {
        id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO github_apps
             (id, name, app_id, client_id, client_secret, webhook_secret, private_key, slug,
              manifest_state, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&name)
        .bind(app_id)
        .bind(&client_id)
        .bind(&client_secret)
        .bind(&webhook_secret)
        .bind(&pem)
        .bind(&slug)
        .bind(manifest_state)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
        id
    };

    tracing::info!("Registered GitHub App {} (app_id={})", name, app_id);

    let gh_app = fetch_github_app(&state.db, &final_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.0))?;
    Ok(GitHubAppResponse::from_app(gh_app))
}

// ── GET /webhooks/source/github/install ───────────────────────────────────────

/// GitHub redirects here after the user installs the App on their repos.
///
/// Uses the `state` query param (our manifest_state UUID) to find the exact
/// github_apps row and writes the installation_id onto it. Falls back to the
/// most-recently-created confirmed row if `state` is absent.
pub async fn github_install_callback(
    State(state): State<AppState>,
    Query(params): Query<GitHubInstallQuery>,
) -> Response {
    if let Some(installation_id) = params.installation_id {
        let result = if let Some(ref ms) = params.state {
            // Precise routing: find the row by manifest_state
            sqlx::query(
                "UPDATE github_apps
                 SET installation_id = ?, updated_at = datetime('now')
                 WHERE manifest_state = ?",
            )
            .bind(installation_id)
            .bind(ms)
            .execute(&state.db)
            .await
        } else {
            // Fallback: most-recently-created confirmed (app_id != 0) row
            sqlx::query(
                "UPDATE github_apps
                 SET installation_id = ?, updated_at = datetime('now')
                 WHERE id = (
                     SELECT id FROM github_apps
                     WHERE app_id != 0
                     ORDER BY created_at DESC LIMIT 1
                 )",
            )
            .bind(installation_id)
            .execute(&state.db)
            .await
        };

        match result {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::info!(
                    "Stored GitHub App installation_id={} (state={:?})",
                    installation_id,
                    params.state
                );
            }
            Ok(_) => {
                tracing::warn!(
                    "GitHub install callback: no row matched state={:?}",
                    params.state
                );
            }
            Err(e) => {
                tracing::error!("Failed to store GitHub App installation_id: {}", e);
            }
        }
    }

    // Redirect to the dashboard integrations page
    let domain = {
        let s = state.settings.read().await;
        s.dashboard_domain.clone()
    };
    let redirect = if domain.is_empty() {
        "/integrations/github".to_string()
    } else {
        format!("https://{}/integrations/github", domain)
    };
    Redirect::to(&redirect).into_response()
}

// ── POST /api/github-apps/{id}/install ────────────────────────────────────────

/// Return the GitHub App installation URL for the registered App.
/// Note: the list endpoint already includes `install_url` on each app, so
/// the dashboard can use that directly without this round-trip.
pub async fn get_install_url(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_github_app(&state.db, &id).await?;
    let url = format!("https://github.com/apps/{}/installations/new", app.slug);
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "url": url }))),
    ))
}

// ── PATCH /api/github-apps/{id} ───────────────────────────────────────────────

pub async fn update_github_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateGitHubAppRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_github_app(&state.db, &id).await?;

    if let Some(ref name) = payload.name {
        sqlx::query("UPDATE github_apps SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(name)
            .bind(&app.id)
            .execute(&state.db)
            .await?;
    }

    if let Some(installation_id) = payload.installation_id {
        sqlx::query(
            "UPDATE github_apps SET installation_id = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(installation_id)
        .bind(&app.id)
        .execute(&state.db)
        .await?;
    }

    let updated = fetch_github_app(&state.db, &app.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(GitHubAppResponse::from_app(updated))),
    ))
}

// ── DELETE /api/github-apps/{id} ──────────────────────────────────────────────

pub async fn delete_github_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_github_app(&state.db, &id).await?;

    // Detach all apps that reference this GitHub App registration
    sqlx::query("UPDATE apps SET github_app_id = NULL WHERE github_app_id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    sqlx::query("DELETE FROM github_apps WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    tracing::info!("Deleted GitHub App registration {} ({})", app.name, app.id);

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "deleted": app.id,
            "name": app.name
        }))),
    ))
}

// ── GET /api/github-apps/{id}/repos ──────────────────────────────────────────

/// List repositories accessible via the GitHub App installation.
pub async fn list_repos(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_github_app(&state.db, &id).await?;

    let token = generate_installation_token(&app)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let client = reqwest::Client::new();
    let mut repos: Vec<GitHubRepoResponse> = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!(
            "https://api.github.com/installation/repositories?per_page=100&page={}",
            page
        );
        let resp: reqwest::Response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "tond-server")
            .send()
            .await
            .map_err(|e: reqwest::Error| {
                AppError(TondError::Internal(format!("GitHub API error: {}", e)))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body: String = resp.text().await.unwrap_or_default();
            return Err(AppError(TondError::Internal(format!(
                "GitHub API returned {}: {}",
                status, body
            ))));
        }

        let json: serde_json::Value = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e: reqwest::Error| AppError(TondError::Internal(e.to_string())))?;

        let Some(arr) = json["repositories"].as_array() else {
            break;
        };

        if arr.is_empty() {
            break;
        }

        for repo in arr {
            repos.push(GitHubRepoResponse {
                id: repo["id"].as_i64().unwrap_or(0),
                full_name: repo["full_name"].as_str().unwrap_or("").to_string(),
                name: repo["name"].as_str().unwrap_or("").to_string(),
                private: repo["private"].as_bool().unwrap_or(false),
                clone_url: repo["clone_url"].as_str().unwrap_or("").to_string(),
                ssh_url: repo["ssh_url"].as_str().unwrap_or("").to_string(),
                default_branch: repo["default_branch"]
                    .as_str()
                    .unwrap_or("main")
                    .to_string(),
            });
        }

        // Check if there are more pages
        let total: i64 = json["total_count"].as_i64().unwrap_or(0);
        if repos.len() as i64 >= total {
            break;
        }
        page += 1;
    }

    Ok((StatusCode::OK, Json(ApiResponse::ok(repos))))
}

// ── GET /api/github-apps/{id}/repos/{owner}/{repo}/branches ──────────────────

pub async fn list_branches(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path((id, owner, repo)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_github_app(&state.db, &id).await?;

    let token = generate_installation_token(&app)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/{}/{}/branches?per_page=100",
        owner, repo
    );
    let resp: reqwest::Response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "tond-server")
        .send()
        .await
        .map_err(|e: reqwest::Error| {
            AppError(TondError::Internal(format!("GitHub API error: {}", e)))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        return Err(AppError(TondError::Internal(format!(
            "GitHub API returned {}: {}",
            status, body
        ))));
    }

    let json: serde_json::Value = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e: reqwest::Error| AppError(TondError::Internal(e.to_string())))?;

    let branches: Vec<GitHubBranchResponse> = json
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|b| {
            b["name"].as_str().map(|name: &str| GitHubBranchResponse {
                name: name.to_string(),
            })
        })
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(branches))))
}

// ── GET /api/apps/{app_id}/github-prs ────────────────────────────────────────

/// Response for a single open pull request.
#[derive(Serialize)]
pub struct OpenPRResponse {
    pub number: i64,
    pub title: String,
    pub head_sha: String,
    pub head_ref: String,
    pub pr_url: String,
}

/// Parse `owner` and `repo` from a GitHub source URL (HTTPS or SSH).
///
/// Examples:
///   `https://github.com/org/repo`      → ("org", "repo")
///   `https://github.com/org/repo.git`  → ("org", "repo")
///   `git@github.com:org/repo.git`      → ("org", "repo")
fn parse_github_owner_repo(source_url: &str) -> Option<(String, String)> {
    let path = source_url
        .strip_prefix("https://github.com/")
        .or_else(|| source_url.strip_prefix("git@github.com:"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

/// List open pull requests for the GitHub repo linked to an app.
pub async fn list_open_prs(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Load app
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&app_id)
    .bind(&app_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::AppNotFound(app_id.clone())))?;

    let github_app_id = app
        .github_app_id
        .ok_or_else(|| AppError(TondError::InvalidInput("No GitHub App linked".to_string())))?;

    let source_url = app
        .source_url
        .ok_or_else(|| AppError(TondError::InvalidInput("App has no source URL".to_string())))?;

    let (owner, repo) = parse_github_owner_repo(&source_url).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "Cannot parse GitHub owner/repo from source URL: {}",
            source_url
        )))
    })?;

    let gh_app = fetch_github_app(&state.db, &github_app_id).await?;

    let token = generate_installation_token(&gh_app)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls?state=open&per_page=100",
        owner, repo
    );
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "tond-server")
        .send()
        .await
        .map_err(|e| AppError(TondError::Internal(format!("GitHub API error: {}", e))))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError(TondError::Internal(format!(
            "GitHub API returned {}: {}",
            status, body
        ))));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let prs: Vec<OpenPRResponse> = json
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|pr| {
            let number = pr["number"].as_i64()?;
            let title = pr["title"].as_str()?.to_string();
            let head_sha = pr["head"]["sha"].as_str()?.to_string();
            let head_ref = pr["head"]["ref"].as_str()?.to_string();
            let pr_url = pr["html_url"].as_str()?.to_string();
            Some(OpenPRResponse {
                number,
                title,
                head_sha,
                head_ref,
                pr_url,
            })
        })
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(prs))))
}

// ── PUT /api/apps/{app_id}/github-app ─────────────────────────────────────────
/// Link or unlink a GitHub App from an app.
pub async fn set_app_github_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<SetGitHubAppRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate the github_app_id exists if provided
    if let Some(ref github_app_id) = payload.github_app_id {
        let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM github_apps WHERE id = ?")
            .bind(github_app_id)
            .fetch_optional(&state.db)
            .await?;
        if exists.is_none() {
            return Err(AppError(TondError::InvalidInput(format!(
                "GitHub App {} not found",
                github_app_id
            ))));
        }
    }

    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&app_id)
    .bind(&app_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::AppNotFound(app_id.clone())))?;

    sqlx::query("UPDATE apps SET github_app_id = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&payload.github_app_id)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    let updated = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps WHERE id = ?")
        .bind(&app.id)
        .fetch_one(&state.db)
        .await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(tond_common::AppResponse::from(updated))),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::to_bytes,
        extract::{Path, Query, State},
        response::IntoResponse,
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{GitHubInstallQuery, ServerSettings, SetGitHubAppRequest};

    use tond_common::GitHubApp;

    use super::*;
    use crate::{
        auth::{RateLimiter, RequireAuth, rate_limit::RateLimit},
        state::AppState,
    };

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db).await.unwrap_or_else(|err| {
            panic!("failed to run migrations for GitHub handler tests: {err}")
        });

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings {
                server_name: "tond".to_string(),
                dashboard_domain: "dashboard.example.com".to_string(),
            })),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_github_app(state: &AppState, id: &str, manifest_state: Option<&str>) {
        sqlx::query(
            "INSERT INTO github_apps (id, name, app_id, client_id, client_secret, webhook_secret, private_key, installation_id, slug, created_at, updated_at, manifest_state)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'), ?)",
        )
        .bind(id)
        .bind("Tond GitHub App")
        .bind(123_i64)
        .bind("client-id")
        .bind("client-secret")
        .bind("webhook-secret")
        .bind("private-key")
        .bind(Option::<i64>::None)
        .bind("tond-app")
        .bind(manifest_state)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed GitHub app row: {err}"));
    }

    async fn seed_source_app(state: &AppState, id: &str, name: &str) {
        let app = tond_common::App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://github.com/org/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&app.name)
        .bind(&app.domain)
        .bind(&app.source_type)
        .bind(&app.source_url)
        .bind(&app.branch)
        .bind(app.container_port)
        .bind(&app.container_id)
        .bind(app.host_port)
        .bind(app.memory_limit)
        .bind(app.cpu_limit)
        .bind(&app.status)
        .bind(&app.env_vars)
        .bind(&app.health_check_path)
        .bind(app.health_check_interval)
        .bind(app.health_failure_threshold)
        .bind(app.health_consecutive_failures)
        .bind(&app.volumes)
        .bind(app.drain_timeout)
        .bind(&app.build_method)
        .bind(&app.webhook_secret)
        .bind(app.auto_deploy)
        .bind(&app.watch_branch)
        .bind(&app.deploy_key_pub)
        .bind(&app.project_id)
        .bind(&app.registry_credential_id)
        .bind(&app.github_app_id)
        .bind(&app.gitlab_token_id)
        .bind(app.preview_enabled)
        .bind(&app.preview_excluded_keys)
        .bind(&app.preview_env_vars)
        .bind(&app.dockerfile_content)
        .bind(&app.compose_content)
        .bind(&app.base_dir)
        .bind(app.static_build)
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed source app row: {err}"));
    }

    fn sample_github_app() -> GitHubApp {
        GitHubApp {
            id: "gh-app-1".to_string(),
            name: "Tond GitHub App".to_string(),
            app_id: 123,
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            webhook_secret: "webhook-secret".to_string(),
            private_key: "not-a-real-key".to_string(),
            installation_id: Some(456),
            slug: "tond-app".to_string(),
            created_at: "2026-04-08T12:00:00Z".to_string(),
            updated_at: "2026-04-08T12:00:00Z".to_string(),
            manifest_state: Some("state-1".to_string()),
        }
    }

    #[test]
    fn parse_github_owner_repo_accepts_https_and_ssh_urls_and_rejects_invalid_input() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/tond-dev/mvp-dev.git"),
            Some(("tond-dev".to_string(), "mvp-dev".to_string())),
            "parse_github_owner_repo should extract owner/repo from HTTPS clone URLs"
        );
        assert_eq!(
            parse_github_owner_repo("git@github.com:tond-dev/mvp-dev.git"),
            Some(("tond-dev".to_string(), "mvp-dev".to_string())),
            "parse_github_owner_repo should extract owner/repo from SSH clone URLs"
        );
        assert_eq!(
            parse_github_owner_repo("https://example.com/tond-dev/mvp-dev.git"),
            None,
            "parse_github_owner_repo should reject non-GitHub URLs"
        );
        assert_eq!(
            parse_github_owner_repo("https://github.com/tond-dev"),
            None,
            "parse_github_owner_repo should reject URLs without a repository segment"
        );
    }

    #[test]
    fn github_app_response_marks_installation_state_and_builds_install_url() {
        let response = GitHubAppResponse::from_app(sample_github_app());

        assert!(
            response.installed,
            "GitHubAppResponse should mark registrations with an installation_id as installed"
        );
        assert_eq!(
            response.install_url, "https://github.com/apps/tond-app/installations/new",
            "GitHubAppResponse should build the GitHub installation URL from the app slug"
        );
    }

    #[tokio::test]
    async fn get_manifest_url_creates_stub_row_and_returns_manifest_payload() {
        let state = test_state().await;

        let response = get_manifest_url(RateLimit, RequireAuth, State(state.clone()))
            .await
            .unwrap_or_else(|_| {
                panic!("get_manifest_url should succeed when dashboard_domain is configured")
            })
            .into_response();
        let body = response_json(response).await;

        assert!(
            body["data"]["github_url"]
                .as_str()
                .unwrap_or_default()
                .starts_with("https://github.com/settings/apps/new?state="),
            "get_manifest_url should return the GitHub manifest bootstrap URL including a generated state token"
        );

        let stub: (String, i64, Option<String>) =
            sqlx::query_as("SELECT name, app_id, manifest_state FROM github_apps LIMIT 1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to load inserted GitHub stub row: {err}"));
        assert_eq!(
            stub.0, "pending",
            "get_manifest_url should create a pending stub row before the callback completes"
        );
        assert_eq!(
            stub.1, 0,
            "get_manifest_url should mark stub rows with app_id=0 until GitHub completes the manifest exchange"
        );
        assert!(
            stub.2.is_some(),
            "get_manifest_url should persist the manifest_state needed to correlate callback rows"
        );
    }

    #[tokio::test]
    async fn github_install_callback_updates_matching_row_by_state() {
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1", Some("state-123")).await;

        let response = github_install_callback(
            State(state.clone()),
            Query(GitHubInstallQuery {
                installation_id: Some(9876),
                setup_action: Some("install".to_string()),
                state: Some("state-123".to_string()),
            }),
        )
        .await;

        assert_eq!(
            response.status(),
            axum::http::StatusCode::SEE_OTHER,
            "github_install_callback should redirect the browser back to the dashboard integrations page"
        );

        let installation_id: Option<i64> =
            sqlx::query_scalar("SELECT installation_id FROM github_apps WHERE id = ?")
                .bind("gh-app-1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to load updated installation_id: {err}"));
        assert_eq!(
            installation_id,
            Some(9876),
            "github_install_callback should update the GitHub app row that matches the callback state"
        );
    }

    #[tokio::test]
    async fn set_and_delete_github_app_updates_app_links() {
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1", Some("state-123")).await;
        seed_source_app(&state, "app-1", "frontend").await;

        let response = set_app_github_app(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path("app-1".to_string()),
            Json(SetGitHubAppRequest {
                github_app_id: Some("gh-app-1".to_string()),
            }),
        )
        .await
        .unwrap_or_else(|_| {
            panic!("set_app_github_app should succeed for existing app and GitHub App rows")
        })
        .into_response();
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["github_app_id"],
            serde_json::Value::String("gh-app-1".to_string()),
            "set_app_github_app should persist the attached GitHub App id on the app row"
        );

        delete_github_app(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path("gh-app-1".to_string()),
        )
        .await
        .unwrap_or_else(|_| {
            panic!("delete_github_app should succeed for an existing GitHub App row")
        });

        let linked_github_app_id: Option<String> =
            sqlx::query_scalar("SELECT github_app_id FROM apps WHERE id = ?")
                .bind("app-1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to load app after GitHub App deletion: {err}")
                });
        assert_eq!(
            linked_github_app_id, None,
            "delete_github_app should detach the deleted GitHub App from all linked apps"
        );
    }
}
