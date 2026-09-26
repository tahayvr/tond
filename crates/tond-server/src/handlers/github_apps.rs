// handlers/github_apps.rs — GitHub App integration (Phase 4.4)
//
// Implements the full GitHub App Manifest flow:
//
//   GET  /api/github-apps
//     List all registered GitHub App registrations.
//
//   POST /api/github-apps/manifest
//     Generate a GitHub App manifest and the `github.com/settings/apps/new?state=`
//     URL the dashboard POSTs it to. Requires `dashboard_domain` and a logged-in
//     admin session: a pending `manifest` flow (github_app_flows) is created
//     for that admin and expires after FLOW_TTL_MINUTES.
//
//   GET  /api/github-apps/callback?code=<code>&state=<state>
//     GitHub redirects here after App creation. The state must match a pending
//     manifest flow owned by the admin whose session cookie comes with the
//     request; the flow is consumed (single use) before the one-time code is
//     exchanged at `POST https://api.github.com/app-manifests/{code}/conversions`
//     and the App is stored. Otherwise nothing is stored.
//
//   GET  /webhooks/source/github/install?installation_id=<id>&setup_action=<action>&state=<state>
//     GitHub redirects here after the user installs the App on their repos.
//     Stores `installation_id` only for a pending `install` flow owned by the
//     session's admin. There is no fallback for requests without state.
//
//   POST /api/github-apps/{id}/install
//     Start an installation flow (admin session required) and return the
//     GitHub installation URL carrying its state.
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
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Serialize;
use tond_common::{
    ApiResponse, CompleteGitHubFlowRequest, GitHubApp, GitHubAppResponse, GitHubBranchResponse,
    GitHubCallbackQuery, GitHubInstallQuery, GitHubRepoResponse, SetGitHubAppRequest, TondError,
    UpdateGitHubAppRequest, User, generate_raw_key,
};

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    handlers::auth::{extract_session_token, validate_session},
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

// ── Pending flows (manifest registration + installation) ─────────────────────

/// How long a pending manifest/install flow stays valid.
const FLOW_TTL_MINUTES: i64 = 60;

/// The admin logged in to the dashboard in this browser, from the session
/// cookie. GitHub App flows are browser round-trips through github.com, so
/// they are bound to a dashboard session rather than an API key or the CLI
/// socket: the callback must come back from the same admin's browser.
async fn session_admin(state: &AppState, headers: &HeaderMap) -> Option<User> {
    let token = extract_session_token(headers)?;
    let user = validate_session(&state.db, &token).await.ok()??;
    (user.role == "admin").then_some(user)
}

/// Create a pending flow owned by `user_id`; returns its state token.
async fn create_flow(
    db: &sqlx::SqlitePool,
    kind: &str,
    github_app_id: Option<&str>,
    user_id: &str,
) -> Result<String, sqlx::Error> {
    let now = chrono::Utc::now();
    // Opportunistic cleanup keeps the table from growing with abandoned flows.
    sqlx::query("DELETE FROM github_app_flows WHERE expires_at < ?")
        .bind(now.to_rfc3339())
        .execute(db)
        .await?;

    let flow_state = generate_raw_key();
    sqlx::query(
        "INSERT INTO github_app_flows (state, kind, github_app_id, user_id, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&flow_state)
    .bind(kind)
    .bind(github_app_id)
    .bind(user_id)
    .bind((now + chrono::Duration::minutes(FLOW_TTL_MINUTES)).to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(db)
    .await?;
    Ok(flow_state)
}

/// Atomically consume a pending, unexpired flow of `kind` owned by `user_id`.
/// Returns the flow's `github_app_id` column (None for manifest flows), or
/// `Err(())` when no such flow exists. A state can only be used once.
async fn consume_flow(
    db: &sqlx::SqlitePool,
    flow_state: &str,
    kind: &str,
    user_id: &str,
) -> Result<Result<Option<String>, ()>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "DELETE FROM github_app_flows
         WHERE state = ? AND kind = ? AND user_id = ? AND expires_at > ?
         RETURNING github_app_id",
    )
    .bind(flow_state)
    .bind(kind)
    .bind(user_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id,)| id).ok_or(()))
}

/// Redirect back to the dashboard's GitHub integration page, optionally with
/// an `error` code the page turns into a message.
async fn redirect_to_integrations(state: &AppState, error: Option<&str>) -> Response {
    let domain = state.settings.read().await.dashboard_domain.clone();
    let base = if domain.is_empty() {
        "/integrations/github".to_string()
    } else {
        format!("https://{}/integrations/github", domain)
    };
    let target = match error {
        Some(code) => format!("{base}?error={code}"),
        None => base,
    };
    Redirect::to(&target).into_response()
}

// ── POST /api/github-apps/manifest ───────────────────────────────────────────

/// Returns the manifest and the GitHub URL the dashboard POSTs it to.
///
/// A pending `manifest` flow is created for the logged-in admin. Its state
/// token rides along on `github.com/settings/apps/new?state=<state>`, GitHub
/// echoes it to the callback, and only that admin's browser session can
/// complete the registration, once, within FLOW_TTL_MINUTES.
pub async fn get_manifest_url(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let Some(admin) = session_admin(&state, &headers).await else {
        return Err(AppError(TondError::InvalidInput(
            "connecting a GitHub App must be started from the dashboard by a logged-in admin"
                .to_string(),
        )));
    };

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
    let manifest_state = create_flow(&state.db, "manifest", None, &admin.id).await?;

    // GitHub validates that redirect_url, callback_urls, setup_url, and
    // hook_attributes.url are absolute URLs with no query strings. The state
    // is passed separately as a query param on the POST action URL
    // (github.com/settings/apps/new?state=<state>) and GitHub echoes it back
    // in the final redirect.
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

// ── GitHub App flow completion ────────────────────────────────────────────────
//
// GitHub sends the browser back to Tond with a top-level cross-site redirect.
// The session cookie is SameSite=Strict, so it is NOT sent on that request.
// The GET callbacks therefore only bounce the browser to the dashboard page
// /integrations/github/complete, which finishes the flow with a same-origin
// POST /api/github-apps/complete: that request carries the admin's session
// cookie and the CSRF header, and consumes the single-use state bound to that
// admin. Nothing is stored before that POST.

/// Where GitHub's redirect is sent: the dashboard page that completes the flow.
async fn redirect_to_completion(state: &AppState, query: &[(&str, String)]) -> Response {
    let domain = state.settings.read().await.dashboard_domain.clone();
    let base = if domain.is_empty() {
        "/integrations/github/complete".to_string()
    } else {
        format!("https://{}/integrations/github/complete", domain)
    };
    let qs = query
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    Redirect::to(&format!("{base}?{qs}")).into_response()
}

/// GET /api/github-apps/callback?code=<code>&state=<state>
///
/// GitHub redirects here after App creation. Hands off to the dashboard.
pub async fn github_app_callback(
    State(state): State<AppState>,
    Query(params): Query<GitHubCallbackQuery>,
) -> Response {
    let mut query = vec![("kind", "manifest".to_string())];
    if let Some(code) = params.code {
        query.push(("code", code));
    }
    if let Some(flow_state) = params.state {
        query.push(("state", flow_state));
    }
    redirect_to_completion(&state, &query).await
}

/// GET /webhooks/source/github/install?installation_id=..&state=..
///
/// GitHub redirects here after the App is installed. Redirects without state
/// (e.g. `setup_on_update` after changing the repo selection on github.com)
/// change nothing and just return to the integrations page.
pub async fn github_install_callback(
    State(state): State<AppState>,
    Query(params): Query<GitHubInstallQuery>,
) -> Response {
    let (Some(installation_id), Some(flow_state)) = (
        params.installation_id,
        params.state.filter(|s| !s.is_empty()),
    ) else {
        return redirect_to_integrations(&state, None).await;
    };
    redirect_to_completion(
        &state,
        &[
            ("kind", "install".to_string()),
            ("installation_id", installation_id.to_string()),
            ("state", flow_state),
        ],
    )
    .await
}

/// POST /api/github-apps/complete — body `{kind, state, code?, installation_id?}`.
///
/// Called by the dashboard page GitHub's redirect lands on. Returns 200 when
/// the flow completed, otherwise 400 with an error code the page explains:
/// missing_state, not_logged_in, invalid_state, missing_code, exchange_failed,
/// internal.
pub async fn complete_github_flow(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteGitHubFlowRequest>,
) -> Response {
    let result = match req.kind.as_str() {
        "manifest" => {
            finish_manifest_flow(
                &state,
                &headers,
                GitHubCallbackQuery {
                    code: req.code,
                    state: req.state,
                },
            )
            .await
        }
        "install" => {
            finish_install_flow(
                &state,
                &headers,
                GitHubInstallQuery {
                    installation_id: req.installation_id,
                    setup_action: None,
                    state: req.state,
                },
            )
            .await
        }
        _ => Err("invalid_state"),
    };
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "completed": true }))),
        )
            .into_response(),
        Err(code) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(code.to_string())),
        )
            .into_response(),
    }
}

/// Complete a pending `manifest` flow: only the admin who started it, with
/// their session, once, within FLOW_TTL_MINUTES. The state is consumed before
/// the code is exchanged, so an attacker cannot make Tond register an App
/// they control.
async fn finish_manifest_flow(
    state: &AppState,
    headers: &HeaderMap,
    params: GitHubCallbackQuery,
) -> Result<(), &'static str> {
    let Some(flow_state) = params.state.as_deref().filter(|s| !s.is_empty()) else {
        tracing::warn!("GitHub App completion without state — refused");
        return Err("missing_state");
    };
    let Some(admin) = session_admin(state, headers).await else {
        tracing::warn!("GitHub App completion without an admin session — refused");
        return Err("not_logged_in");
    };

    match consume_flow(&state.db, flow_state, "manifest", &admin.id).await {
        Ok(Ok(_)) => {}
        Ok(Err(())) => {
            tracing::warn!("GitHub App completion with unknown, expired or used state — refused");
            return Err("invalid_state");
        }
        Err(e) => {
            tracing::error!("GitHub App completion: failed to load flow: {}", e);
            return Err("internal");
        }
    }

    let Some(code) = params.code.as_deref().filter(|c| is_valid_manifest_code(c)) else {
        return Err("missing_code");
    };

    exchange_manifest_code(state, code)
        .await
        .map(|_| ())
        .map_err(|e| {
            tracing::error!("GitHub App manifest exchange failed: {}", e);
            "exchange_failed"
        })
}

/// Manifest codes are short alphanumeric tokens; anything else is refused
/// before it is interpolated into the GitHub API path.
fn is_valid_manifest_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 256
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Exchange a one-time manifest code for App credentials and store them.
async fn exchange_manifest_code(state: &AppState, code: &str) -> anyhow::Result<GitHubAppResponse> {
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
        .filter(|s| !s.is_empty())
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
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO github_apps
         (id, name, app_id, client_id, client_secret, webhook_secret, private_key, slug,
          created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(app_id)
    .bind(&client_id)
    .bind(&client_secret)
    .bind(&webhook_secret)
    .bind(&pem)
    .bind(&slug)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    tracing::info!("Registered GitHub App {} (app_id={})", name, app_id);

    let gh_app = fetch_github_app(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.0))?;
    Ok(GitHubAppResponse::from_app(gh_app))
}

/// Complete a pending `install` flow started (via `POST /api/github-apps/{id}/install`)
/// by the admin whose session accompanies the request. A request without state
/// changes nothing.
async fn finish_install_flow(
    state: &AppState,
    headers: &HeaderMap,
    params: GitHubInstallQuery,
) -> Result<(), &'static str> {
    let (Some(installation_id), Some(flow_state)) = (
        params.installation_id,
        params.state.as_deref().filter(|s| !s.is_empty()),
    ) else {
        return Ok(());
    };
    let Some(admin) = session_admin(state, headers).await else {
        tracing::warn!("GitHub install completion without an admin session — refused");
        return Err("not_logged_in");
    };

    let github_app_id = match consume_flow(&state.db, flow_state, "install", &admin.id).await {
        Ok(Ok(Some(id))) => id,
        Ok(_) => {
            tracing::warn!(
                "GitHub install completion with unknown, expired or used state — refused"
            );
            return Err("invalid_state");
        }
        Err(e) => {
            tracing::error!("GitHub install completion: failed to load flow: {}", e);
            return Err("internal");
        }
    };

    sqlx::query(
        "UPDATE github_apps SET installation_id = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(installation_id)
    .bind(&github_app_id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("Failed to store GitHub App installation_id: {}", e);
        "internal"
    })?;
    tracing::info!(
        "Stored GitHub App installation_id={} for {}",
        installation_id,
        github_app_id
    );
    Ok(())
}

// ── POST /api/github-apps/{id}/install ────────────────────────────────────────

/// Start an installation flow: returns the GitHub installation URL carrying
/// a single-use state bound to the logged-in admin.
pub async fn get_install_url(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let Some(admin) = session_admin(&state, &headers).await else {
        return Err(AppError(TondError::InvalidInput(
            "installing a GitHub App must be started from the dashboard by a logged-in admin"
                .to_string(),
        )));
    };
    let app = fetch_github_app(&state.db, &id).await?;
    let flow_state = create_flow(&state.db, "install", Some(&app.id), &admin.id).await?;
    let url = format!(
        "https://github.com/apps/{}/installations/new?state={}",
        urlencoding::encode(&app.slug),
        flow_state
    );
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
    use tond_common::{
        GitHubCallbackQuery, GitHubInstallQuery, ServerSettings, SetGitHubAppRequest,
    };

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
                preview_domain: String::new(),
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

    async fn seed_github_app(state: &AppState, id: &str) {
        sqlx::query(
            "INSERT INTO github_apps (id, name, app_id, client_id, client_secret, webhook_secret, private_key, installation_id, slug, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
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
    fn github_app_response_marks_installation_state() {
        let response = GitHubAppResponse::from_app(sample_github_app());

        assert!(
            response.installed,
            "GitHubAppResponse should mark registrations with an installation_id as installed"
        );
    }

    #[test]
    fn manifest_codes_must_be_plain_tokens() {
        assert!(is_valid_manifest_code("a1b2c3d4e5"));
        assert!(!is_valid_manifest_code(""));
        assert!(!is_valid_manifest_code("../../user"));
        assert!(!is_valid_manifest_code("abc?x=1"));
        assert!(!is_valid_manifest_code(&"a".repeat(300)));
    }

    /// Insert a user and a live session; returns the session cookie header.
    async fn seed_session(state: &AppState, user_id: &str, role: &str) -> HeaderMap {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, 'x', ?, ?, ?)",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.com"))
        .bind(role)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));

        let session_id = format!("session-{user_id}");
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(tond_common::hash_key(&session_id))
        .bind(user_id)
        .bind((now + chrono::Duration::hours(1)).to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session: {err}"));

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            axum::http::HeaderValue::from_str(&format!("__Host-tond_session={session_id}"))
                .unwrap_or_else(|err| panic!("invalid cookie header: {err}")),
        );
        headers
    }

    fn location(response: &Response) -> String {
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn github_redirects_hand_off_to_the_dashboard_without_storing_anything() {
        // The browser arrives from github.com without the SameSite=Strict
        // session cookie, so the GET callbacks must not act on their own.
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1").await;

        let response = github_app_callback(
            State(state.clone()),
            Query(GitHubCallbackQuery {
                code: Some("abc123".to_string()),
                state: Some("s t&x".to_string()),
            }),
        )
        .await;
        assert_eq!(
            location(&response),
            "https://dashboard.example.com/integrations/github/complete?kind=manifest&code=abc123&state=s%20t%26x"
        );

        let response = github_install_callback(
            State(state.clone()),
            Query(GitHubInstallQuery {
                installation_id: Some(42),
                setup_action: Some("install".to_string()),
                state: Some("st".to_string()),
            }),
        )
        .await;
        assert_eq!(
            location(&response),
            "https://dashboard.example.com/integrations/github/complete?kind=install&installation_id=42&state=st"
        );
        assert_eq!(installation_id(&state, "gh-app-1").await, None);
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM github_apps WHERE installation_id IS NOT NULL"
            )
            .await,
            0
        );
    }

    async fn count(state: &AppState, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("count query failed: {err}"))
    }

    async fn manifest_state_for(state: &AppState, headers: HeaderMap) -> String {
        let response = get_manifest_url(RateLimit, RequireAuth, State(state.clone()), headers)
            .await
            .unwrap_or_else(|_| panic!("get_manifest_url should succeed for an admin session"))
            .into_response();
        let body = response_json(response).await;
        body["data"]["github_url"]
            .as_str()
            .and_then(|u| u.strip_prefix("https://github.com/settings/apps/new?state="))
            .unwrap_or_else(|| panic!("github_url should carry the flow state: {body}"))
            .to_string()
    }

    #[tokio::test]
    async fn get_manifest_url_creates_a_pending_flow_for_the_admin_only() {
        let state = test_state().await;
        let admin = seed_session(&state, "admin-1", "admin").await;

        let flow_state = manifest_state_for(&state, admin).await;
        assert_eq!(flow_state.len(), 64);

        let (kind, user_id): (String, String) =
            sqlx::query_as("SELECT kind, user_id FROM github_app_flows WHERE state = ?")
                .bind(&flow_state)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("the flow row should exist: {err}"));
        assert_eq!((kind.as_str(), user_id.as_str()), ("manifest", "admin-1"));
        assert_eq!(
            count(&state, "SELECT COUNT(*) FROM github_apps").await,
            0,
            "no github_apps row may exist before GitHub confirms the registration"
        );

        // API keys / the CLI socket (no session) and non-admin users cannot start it.
        let member = seed_session(&state, "member-1", "member").await;
        for headers in [HeaderMap::new(), member] {
            assert!(
                get_manifest_url(RateLimit, RequireAuth, State(state.clone()), headers)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn github_app_callback_refuses_codes_without_a_pending_flow() {
        let state = test_state().await;
        let admin = seed_session(&state, "admin-1", "admin").await;

        // The original attack: a bare code (no state) from anyone.
        let response = finish_manifest_flow(
            &state,
            &HeaderMap::new(),
            GitHubCallbackQuery {
                code: Some("attackercode".to_string()),
                state: None,
            },
        )
        .await;
        assert_eq!(response, Err("missing_state"));

        // A made-up state, even from a logged-in admin.
        let response = finish_manifest_flow(
            &state,
            &admin.clone(),
            GitHubCallbackQuery {
                code: Some("attackercode".to_string()),
                state: Some("made-up".to_string()),
            },
        )
        .await;
        assert_eq!(response, Err("invalid_state"));

        // A real state, but the callback arrives without that admin's session
        // (e.g. an attacker sending the victim's state with their own code).
        let flow_state = manifest_state_for(&state, admin.clone()).await;
        let response = finish_manifest_flow(
            &state,
            &HeaderMap::new(),
            GitHubCallbackQuery {
                code: Some("attackercode".to_string()),
                state: Some(flow_state.clone()),
            },
        )
        .await;
        assert_eq!(response, Err("not_logged_in"));

        // Another admin's session cannot complete it either.
        let other = seed_session(&state, "admin-2", "admin").await;
        let response = finish_manifest_flow(
            &state,
            &other,
            GitHubCallbackQuery {
                code: Some("attackercode".to_string()),
                state: Some(flow_state.clone()),
            },
        )
        .await;
        assert_eq!(response, Err("invalid_state"));

        assert_eq!(count(&state, "SELECT COUNT(*) FROM github_apps").await, 0);
    }

    #[tokio::test]
    async fn github_app_callback_state_is_single_use_and_expires() {
        let state = test_state().await;
        let admin = seed_session(&state, "admin-1", "admin").await;

        // The owner's callback consumes the state before the code is used. A
        // malformed code stops before any network call.
        let flow_state = manifest_state_for(&state, admin.clone()).await;
        let response = finish_manifest_flow(
            &state,
            &admin.clone(),
            GitHubCallbackQuery {
                code: Some("../bad".to_string()),
                state: Some(flow_state.clone()),
            },
        )
        .await;
        assert_eq!(response, Err("missing_code"));

        let response = finish_manifest_flow(
            &state,
            &admin.clone(),
            GitHubCallbackQuery {
                code: Some("../bad".to_string()),
                state: Some(flow_state),
            },
        )
        .await;
        assert_eq!(
            response,
            Err("invalid_state"),
            "a state can only be used once"
        );

        // An expired flow is refused.
        let expired_state = manifest_state_for(&state, admin.clone()).await;
        sqlx::query("UPDATE github_app_flows SET expires_at = ? WHERE state = ?")
            .bind((chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339())
            .bind(&expired_state)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to expire flow: {err}"));
        let response = finish_manifest_flow(
            &state,
            &admin,
            GitHubCallbackQuery {
                code: Some("code".to_string()),
                state: Some(expired_state),
            },
        )
        .await;
        assert_eq!(response, Err("invalid_state"));
    }

    async fn install_state_for(state: &AppState, headers: HeaderMap, id: &str) -> String {
        let response = get_install_url(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            headers,
            Path(id.to_string()),
        )
        .await
        .unwrap_or_else(|_| panic!("get_install_url should succeed for an admin session"))
        .into_response();
        let body = response_json(response).await;
        body["data"]["url"]
            .as_str()
            .and_then(|u| {
                u.strip_prefix("https://github.com/apps/tond-app/installations/new?state=")
            })
            .unwrap_or_else(|| panic!("install url should carry the flow state: {body}"))
            .to_string()
    }

    async fn installation_id(state: &AppState, id: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT installation_id FROM github_apps WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load installation_id: {err}"))
    }

    #[tokio::test]
    async fn github_install_callback_requires_the_admins_pending_install_flow() {
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1").await;
        let admin = seed_session(&state, "admin-1", "admin").await;

        // No state (e.g. setup_on_update, or an attacker): nothing changes.
        let response = finish_install_flow(
            &state,
            &admin.clone(),
            GitHubInstallQuery {
                installation_id: Some(1111),
                setup_action: Some("install".to_string()),
                state: None,
            },
        )
        .await;
        assert_eq!(response, Ok(()));
        assert_eq!(installation_id(&state, "gh-app-1").await, None);

        let flow_state = install_state_for(&state, admin.clone(), "gh-app-1").await;

        // Without the admin's session the state is useless.
        let response = finish_install_flow(
            &state,
            &HeaderMap::new(),
            GitHubInstallQuery {
                installation_id: Some(2222),
                setup_action: Some("install".to_string()),
                state: Some(flow_state.clone()),
            },
        )
        .await;
        assert_eq!(response, Err("not_logged_in"));
        assert_eq!(installation_id(&state, "gh-app-1").await, None);

        let response = finish_install_flow(
            &state,
            &admin.clone(),
            GitHubInstallQuery {
                installation_id: Some(9876),
                setup_action: Some("install".to_string()),
                state: Some(flow_state.clone()),
            },
        )
        .await;
        assert_eq!(response, Ok(()));
        assert_eq!(installation_id(&state, "gh-app-1").await, Some(9876));

        // Replaying the same state is refused.
        let response = finish_install_flow(
            &state,
            &admin,
            GitHubInstallQuery {
                installation_id: Some(3333),
                setup_action: Some("install".to_string()),
                state: Some(flow_state),
            },
        )
        .await;
        assert_eq!(response, Err("invalid_state"));
        assert_eq!(installation_id(&state, "gh-app-1").await, Some(9876));
    }

    #[tokio::test]
    async fn manifest_states_cannot_complete_install_flows() {
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1").await;
        let admin = seed_session(&state, "admin-1", "admin").await;
        let manifest_state = manifest_state_for(&state, admin.clone()).await;

        let response = finish_install_flow(
            &state,
            &admin,
            GitHubInstallQuery {
                installation_id: Some(1),
                setup_action: None,
                state: Some(manifest_state),
            },
        )
        .await;
        assert_eq!(response, Err("invalid_state"));
        assert_eq!(installation_id(&state, "gh-app-1").await, None);
    }

    #[tokio::test]
    async fn set_and_delete_github_app_updates_app_links() {
        let state = test_state().await;
        seed_github_app(&state, "gh-app-1").await;
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
