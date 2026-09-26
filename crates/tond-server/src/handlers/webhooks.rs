// handlers/webhooks.rs — Webhook endpoints for GitHub, GitLab, Gitea/Forgejo and generic callers
//
// Per-app endpoints, addressed by app UUID only (never by name):
//
//   POST /api/v1/webhooks/github/{app_id}   X-Hub-Signature-256: sha256=HMAC-SHA256(secret, body)
//   POST /api/v1/webhooks/gitlab/{app_id}   X-Gitlab-Token: <secret>   (constant-time compare)
//   POST /api/v1/webhooks/gitea/{app_id}    X-Gitea-Signature / X-Forgejo-Signature: hex HMAC (push, pull_request)
//   POST /api/v1/webhooks/generic/{app_id}  X-Tond-Signature-256: sha256=<hex HMAC>
//                                        or Authorization: Bearer <secret>
//
// GitHub App endpoint (all repos the App is installed on):
//
//   POST /webhooks/source/github/events  X-Hub-Signature-256 with the App's webhook secret
//
// Every app gets a random webhook secret at creation (`App::new`) and the
// secret can only be replaced or rotated, never cleared: a request without a
// valid signature/token is rejected with 401 before anything else happens.
//
// Replay protection: after verification, the provider's delivery ID
// (X-GitHub-Delivery, Idempotency-Key / X-Gitlab-Event-UUID, X-Gitea-Delivery,
// X-Forgejo-Delivery, X-Tond-Delivery) and — for provider webhooks — a SHA-256
// digest of the body are remembered in `webhook_deliveries` for
// REPLAY_WINDOW_HOURS (capped at MAX_REMEMBERED_DELIVERIES rows). A repeat is
// answered with 409. The digest matters because delivery headers are not
// covered by the signature: an attacker replaying a captured body could
// otherwise just change the header. Generic webhooks skip the digest (CI jobs
// legitimately send identical bodies) and should send X-Tond-Delivery.
// Note that provider "redeliver" buttons reuse the delivery ID and are
// therefore rejected as replays; trigger a manual deploy instead.
//
// Rate limiting: webhook-triggered deploys and previews share a per-app token
// bucket (WEBHOOK_BURST, one token per WEBHOOK_REFILL_SECS), and an app can
// have at most MAX_ACTIVE_PREVIEWS_PER_APP live previews.
//
// If auto_deploy = false, verified push webhooks return 200 but do nothing.

use std::sync::LazyLock;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use tond_common::{ApiResponse, App, GitHubApp, PreviewDeployment, TondError};

use crate::{
    auth::RateLimiter, deploy, docker, domains::preview_host, error::AppError, secrets,
    state::AppState,
};

/// How long delivery IDs and payload digests are remembered.
const REPLAY_WINDOW_HOURS: i64 = 72;
/// Upper bound on remembered deliveries (oldest are dropped first).
const MAX_REMEMBERED_DELIVERIES: i64 = 10_000;
/// Burst of webhook-triggered deploys/previews per app.
const WEBHOOK_BURST: u32 = 10;
/// Seconds per refilled deploy token (sustained rate: 2 per minute).
const WEBHOOK_REFILL_SECS: f64 = 30.0;
/// Live (non-expired) preview deployments allowed per app.
const MAX_ACTIVE_PREVIEWS_PER_APP: i64 = 10;

/// Per-app limiter for webhook-triggered deploys and previews, keyed by app id.
static WEBHOOK_DEPLOY_LIMITER: LazyLock<RateLimiter> =
    LazyLock::new(|| RateLimiter::new(WEBHOOK_BURST, 1.0 / WEBHOOK_REFILL_SECS));

// ── Response helpers ───────────────────────────────────────────────────────

fn ok_response() -> Response {
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

fn info_response(message: &str) -> Response {
    (StatusCode::OK, Json(ApiResponse::<()>::ok_info(message))).into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    crate::error::status_error(status, message)
}

// ── Verification ───────────────────────────────────────────────────────────

/// Which kind of per-app webhook a request arrived on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    GitHub,
    GitLab,
    Gitea,
    Generic,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::GitLab => "gitlab",
            Provider::Gitea => "gitea",
            Provider::Generic => "generic",
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Constant-time byte comparison (length is not secret).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// Verify a hex-encoded HMAC-SHA256 of `body` (constant time via `verify_slice`).
fn verify_hmac_hex(hex_digest: &str, body: &[u8], secret: &str) -> bool {
    let Ok(expected) = hex::decode(hex_digest) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// Validate a GitHub-style `sha256=<hex_digest>` signature header.
fn verify_github_signature(header: &str, body: &[u8], secret: &str) -> bool {
    header
        .strip_prefix("sha256=")
        .is_some_and(|hex_digest| verify_hmac_hex(hex_digest, body, secret))
}

/// Check the provider-specific signature or token against the app's secret.
fn verify_request(provider: Provider, headers: &HeaderMap, body: &[u8], secret: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    match provider {
        Provider::GitHub => header(headers, "x-hub-signature-256")
            .is_some_and(|sig| verify_github_signature(sig, body, secret)),
        Provider::GitLab => header(headers, "x-gitlab-token")
            .is_some_and(|token| constant_time_eq(token.as_bytes(), secret.as_bytes())),
        Provider::Gitea => {
            if let Some(sig) =
                header(headers, "x-gitea-signature").or(header(headers, "x-forgejo-signature"))
            {
                verify_hmac_hex(sig, body, secret)
            } else {
                header(headers, "x-hub-signature-256")
                    .is_some_and(|sig| verify_github_signature(sig, body, secret))
            }
        }
        Provider::Generic => {
            if let Some(sig) = header(headers, "x-tond-signature-256") {
                verify_github_signature(sig, body, secret)
            } else {
                header(headers, "authorization")
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .is_some_and(|token| {
                        constant_time_eq(token.trim().as_bytes(), secret.as_bytes())
                    })
            }
        }
    }
}

/// The provider's per-delivery identifier, if it sent one.
fn delivery_id(provider: Provider, headers: &HeaderMap) -> Option<String> {
    let value = match provider {
        Provider::GitHub => header(headers, "x-github-delivery"),
        Provider::GitLab => {
            header(headers, "idempotency-key").or(header(headers, "x-gitlab-event-uuid"))
        }
        Provider::Gitea => {
            header(headers, "x-gitea-delivery").or(header(headers, "x-forgejo-delivery"))
        }
        Provider::Generic => header(headers, "x-tond-delivery"),
    }?;
    // Bound what we store; real delivery IDs are UUIDs.
    Some(value.chars().take(128).collect())
}

// ── Replay protection ──────────────────────────────────────────────────────

/// Remember `key`; returns false if it was already seen within the window.
/// Also prunes expired entries and enforces the row cap.
async fn remember_delivery(db: &sqlx::SqlitePool, key: &str) -> Result<bool, sqlx::Error> {
    let now = chrono::Utc::now();
    let cutoff = (now - chrono::Duration::hours(REPLAY_WINDOW_HOURS)).to_rfc3339();

    sqlx::query("DELETE FROM webhook_deliveries WHERE received_at < ?")
        .bind(&cutoff)
        .execute(db)
        .await?;

    let inserted =
        sqlx::query("INSERT OR IGNORE INTO webhook_deliveries (key, received_at) VALUES (?, ?)")
            .bind(key)
            .bind(now.to_rfc3339())
            .execute(db)
            .await?
            .rows_affected()
            == 1;

    if inserted {
        sqlx::query(
            "DELETE FROM webhook_deliveries WHERE key IN (
                 SELECT key FROM webhook_deliveries
                 ORDER BY received_at DESC LIMIT -1 OFFSET ?
             )",
        )
        .bind(MAX_REMEMBERED_DELIVERIES)
        .execute(db)
        .await?;
    }

    Ok(inserted)
}

/// Record the delivery ID and (optionally) the body digest under `scope`.
/// Returns false when either was already seen, i.e. the request is a replay.
async fn check_replay(
    db: &sqlx::SqlitePool,
    scope: &str,
    delivery: Option<&str>,
    body: Option<&[u8]>,
) -> Result<bool, sqlx::Error> {
    let mut fresh = true;
    if let Some(id) = delivery {
        fresh &= remember_delivery(db, &format!("{scope}:id:{id}")).await?;
    }
    if let Some(body) = body {
        let digest = hex::encode(Sha256::digest(body));
        fresh &= remember_delivery(db, &format!("{scope}:body:{digest}")).await?;
    }
    Ok(fresh)
}

// ── Per-app limits ─────────────────────────────────────────────────────────

/// Consume one webhook deploy token for `app_id`.
async fn take_deploy_token(app_id: &str) -> bool {
    WEBHOOK_DEPLOY_LIMITER.check(app_id).await
}

/// Check the per-app limits before creating a webhook-triggered preview.
/// Returns an error message when the preview must not be created.
async fn preview_allowed(db: &sqlx::SqlitePool, app_id: &str) -> Result<(), &'static str> {
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM preview_deployments WHERE app_id = ? AND status != 'expired'",
    )
    .bind(app_id)
    .fetch_one(db)
    .await
    .unwrap_or(0);
    if active >= MAX_ACTIVE_PREVIEWS_PER_APP {
        return Err("too many active previews for this app");
    }
    if !take_deploy_token(app_id).await {
        return Err("webhook rate limit exceeded for this app");
    }
    Ok(())
}

/// Accept only `https://` URLs with a host and no embedded credentials.
/// Used for PR/MR links from webhook payloads before they are stored and
/// rendered as links in the dashboard.
pub fn sanitize_provider_url(raw: Option<&str>) -> Option<String> {
    let url = reqwest::Url::parse(raw?.trim()).ok()?;
    if url.scheme() != "https"
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.to_string())
}

// ── Shared deploy trigger ──────────────────────────────────────────────────

/// "GitHub push to main" — the deployment's trigger detail.
fn webhook_trigger_detail(provider: &str, branch: Option<&str>) -> String {
    let name = match provider {
        "github" => "GitHub",
        "github-app" => "GitHub App",
        "gitlab" => "GitLab",
        "gitea" => "Gitea",
        "generic" => "Generic webhook",
        other => other,
    };
    match branch {
        Some(b) => format!("{name} push to {b}"),
        None => name.to_string(),
    }
}

/// Check auto_deploy, the branch filter and the per-app rate limit, then queue
/// a deploy (deploy/queue.rs).
async fn trigger_webhook_deploy(
    state: &AppState,
    app: App,
    provider: &str,
    branch: Option<&str>,
) -> Response {
    if app.auto_deploy == 0 {
        tracing::debug!(
            "[webhook/{}/{}] auto_deploy not enabled — skipping",
            provider,
            app.id
        );
        return info_response("auto_deploy not enabled");
    }

    // Branch filter: if watch_branch is set, only deploy when the pushed branch matches.
    if let Some(watch) = &app.watch_branch
        && let Some(pushed) = branch
        && pushed != watch
    {
        tracing::debug!(
            "[webhook/{}/{}] branch '{}' != watch_branch '{}' — skipping",
            provider,
            app.id,
            pushed,
            watch
        );
        return info_response("branch not watched");
    }

    if !take_deploy_token(&app.id).await {
        tracing::warn!("[webhook/{}/{}] rate limit exceeded", provider, app.id);
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "webhook rate limit exceeded for this app",
        );
    }

    tracing::info!(
        "[webhook/{}/{}] triggering deploy (branch: {:?})",
        provider,
        app.id,
        branch
    );

    // A burst of pushes coalesces in the queue: the newest supersedes a
    // deploy that has not started yet.
    let detail = webhook_trigger_detail(provider, branch);
    let app_id = app.id.clone();
    match deploy::start(
        state,
        app,
        deploy::DeployRequest::source("webhook", Some(detail)),
    )
    .await
    {
        Ok(queued) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(crate::handlers::queued_response(
                "Deployment queued",
                &app_id,
                &queued,
            ))),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[webhook/{}] could not queue deploy: {}", provider, e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not queue the deploy",
            )
        }
    }
}

/// Fetch an app by UUID for webhook use. Names are not accepted: webhook URLs
/// must not be guessable from an app's (public) name.
async fn fetch_app_for_webhook(db: &sqlx::SqlitePool, id: &str) -> Result<App, AppError> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(AppError(TondError::AppNotFound(id.to_string())));
    }
    sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError(TondError::AppNotFound(id.to_string())))
}

/// Load the app, verify the request against its secret and reject replays.
/// Returns the app, or the response to send back.
async fn authenticate_webhook(
    state: &AppState,
    provider: Provider,
    app_id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Result<App, Response>, AppError> {
    let app = fetch_app_for_webhook(&state.db, app_id).await?;
    // A secret that cannot be decrypted verifies nothing (fail closed).
    let secret = secrets::app_webhook_secret(&app)
        .ok()
        .flatten()
        .unwrap_or_default();

    if !verify_request(provider, headers, body, &secret) {
        tracing::warn!(
            "[webhook/{}/{}] rejected: missing or invalid signature",
            provider.as_str(),
            app.id
        );
        return Ok(Err(error_response(
            StatusCode::UNAUTHORIZED,
            "invalid or missing webhook signature",
        )));
    }

    let scope = format!("{}:{}", provider.as_str(), app.id);
    let body_digest = (provider != Provider::Generic).then_some(body);
    let delivery = delivery_id(provider, headers);
    if !check_replay(&state.db, &scope, delivery.as_deref(), body_digest).await? {
        tracing::warn!(
            "[webhook/{}/{}] rejected replayed delivery {:?}",
            provider.as_str(),
            app.id,
            delivery
        );
        return Ok(Err(error_response(
            StatusCode::CONFLICT,
            "duplicate webhook delivery: this delivery was already received and is not processed twice. To deploy again, push a new commit or use Re-deploy in the Tond dashboard.",
        )));
    }

    Ok(Ok(app))
}

/// Extract the short branch name from a push payload (`ref` = "refs/heads/<branch>").
/// Used for GitHub, GitLab and Gitea, which share the field.
fn extract_push_branch(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let r = v.get("ref")?.as_str()?;
    Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
}

// ── GitHub ─────────────────────────────────────────────────────────────────

/// POST /api/v1/webhooks/github/{app_id}
///
/// Requires a valid X-Hub-Signature-256; only "push" events trigger a deploy.
pub async fn github_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let app = match authenticate_webhook(&state, Provider::GitHub, &app_id, &headers, &body).await?
    {
        Ok(app) => app,
        Err(response) => return Ok(response),
    };

    if header(&headers, "x-github-event") != Some("push") {
        // Non-push events (e.g. ping) are accepted without action.
        return Ok(ok_response());
    }

    let branch = extract_push_branch(&body);
    Ok(trigger_webhook_deploy(&state, app, "github", branch.as_deref()).await)
}

// ── GitLab ─────────────────────────────────────────────────────────────────

/// POST /api/v1/webhooks/gitlab/{app_id}
///
/// Requires X-Gitlab-Token to equal the app's secret. "Push Hook" triggers a
/// deploy; "Merge Request Hook" drives preview deployments.
pub async fn gitlab_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let app = match authenticate_webhook(&state, Provider::GitLab, &app_id, &headers, &body).await?
    {
        Ok(app) => app,
        Err(response) => return Ok(response),
    };

    match header(&headers, "x-gitlab-event") {
        Some("Push Hook") => {
            let branch = extract_push_branch(&body);
            Ok(trigger_webhook_deploy(&state, app, "gitlab", branch.as_deref()).await)
        }
        Some("Merge Request Hook") => handle_gitlab_merge_request(&state, app, &body).await,
        // All other GitLab events are accepted without action.
        _ => Ok(ok_response()),
    }
}

// ── Gitea / Forgejo ────────────────────────────────────────────────────────

/// POST /api/v1/webhooks/gitea/{app_id}
///
/// Requires a valid X-Gitea-Signature (or X-Forgejo-Signature). "push" events
/// trigger a deploy; "pull_request" events drive preview deployments.
pub async fn gitea_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let app = match authenticate_webhook(&state, Provider::Gitea, &app_id, &headers, &body).await? {
        Ok(app) => app,
        Err(response) => return Ok(response),
    };

    let event = header(&headers, "x-gitea-event").or(header(&headers, "x-forgejo-event"));
    match event {
        Some("push") => {
            let branch = extract_push_branch(&body);
            Ok(trigger_webhook_deploy(&state, app, "gitea", branch.as_deref()).await)
        }
        Some("pull_request" | "pull_request_sync") => {
            handle_gitea_pull_request(&state, app, &body).await
        }
        _ => Ok(ok_response()),
    }
}

// ── Generic ────────────────────────────────────────────────────────────────

/// POST /api/v1/webhooks/generic/{app_id}
///
/// For CI pipelines and scripts. Authenticate with
/// `X-Tond-Signature-256: sha256=<HMAC-SHA256(secret, body)>` or
/// `Authorization: Bearer <secret>`. Send a unique `X-Tond-Delivery` header to
/// get replay protection. Any authenticated POST triggers a deploy when
/// auto_deploy is enabled.
pub async fn generic_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let app =
        match authenticate_webhook(&state, Provider::Generic, &app_id, &headers, &body).await? {
            Ok(app) => app,
            Err(response) => return Ok(response),
        };

    Ok(trigger_webhook_deploy(&state, app, "generic", None).await)
}

// ── Pull/merge request previews (GitHub App, GitLab, Gitea/Forgejo) ────────
//
// Every forge's event is reduced to a `PrEvent`. Opening, reopening or
// pushing to a pull request replaces its preview (the previous one is
// stopped; its comment id carries over so the comment is edited, not
// duplicated) and reports a pending tond/preview status; the deploy itself
// reports success (deploy/core.rs) or failure. Closing or merging removes
// the previews. Statuses and comments go through forge.rs.

/// What a pull/merge request event asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrAction {
    /// Opened, reopened or new commits: (re)deploy the preview
    Deploy,
    /// Closed or merged: remove the previews
    Close,
    /// Anything else (labels, edits, reviews): nothing to do
    Ignore(String),
}

/// A pull/merge request event, whatever the forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrEvent {
    pub action: PrAction,
    /// PR number (GitLab: the MR iid)
    pub number: i64,
    pub head_sha: Option<String>,
    pub head_ref: Option<String>,
    pub url: Option<String>,
    /// Clone URL of the repository the event is about
    pub repo_url: Option<String>,
    pub gitlab_project_id: Option<i64>,
}

/// A GitHub `pull_request` event.
pub(crate) fn parse_github_pull_request(body: &[u8]) -> Option<PrEvent> {
    let p: serde_json::Value = serde_json::from_slice(body).ok()?;
    let action = p["action"].as_str().unwrap_or_default();
    Some(PrEvent {
        action: match action {
            "opened" | "synchronize" | "reopened" => PrAction::Deploy,
            "closed" => PrAction::Close,
            other => PrAction::Ignore(other.to_string()),
        },
        number: p["number"].as_i64()?,
        head_sha: p["pull_request"]["head"]["sha"]
            .as_str()
            .map(str::to_string),
        head_ref: p["pull_request"]["head"]["ref"]
            .as_str()
            .map(str::to_string),
        url: sanitize_provider_url(p["pull_request"]["html_url"].as_str()),
        repo_url: p["repository"]["clone_url"]
            .as_str()
            .or_else(|| p["repository"]["html_url"].as_str())
            .map(str::to_string),
        gitlab_project_id: None,
    })
}

/// A GitLab `Merge Request Hook` event.
pub(crate) fn parse_gitlab_merge_request(body: &[u8]) -> Option<PrEvent> {
    let p: serde_json::Value = serde_json::from_slice(body).ok()?;
    let attrs = &p["object_attributes"];
    let action = attrs["action"].as_str().unwrap_or_default();
    Some(PrEvent {
        action: match action {
            "open" | "update" | "reopen" => PrAction::Deploy,
            "close" | "merge" => PrAction::Close,
            other => PrAction::Ignore(other.to_string()),
        },
        number: attrs["iid"].as_i64()?,
        head_sha: attrs["last_commit"]["id"].as_str().map(str::to_string),
        head_ref: attrs["source_branch"].as_str().map(str::to_string),
        url: sanitize_provider_url(attrs["url"].as_str()),
        repo_url: p["project"]["http_url_to_repo"]
            .as_str()
            .or_else(|| p["repository"]["homepage"].as_str())
            .map(str::to_string),
        gitlab_project_id: p["project"]["id"].as_i64().filter(|id| *id > 0),
    })
}

/// A Gitea/Forgejo `pull_request` event.
pub(crate) fn parse_gitea_pull_request(body: &[u8]) -> Option<PrEvent> {
    let p: serde_json::Value = serde_json::from_slice(body).ok()?;
    let action = p["action"].as_str().unwrap_or_default();
    Some(PrEvent {
        action: match action {
            "opened" | "reopened" | "synchronized" => PrAction::Deploy,
            "closed" => PrAction::Close,
            other => PrAction::Ignore(other.to_string()),
        },
        number: p["number"]
            .as_i64()
            .or_else(|| p["pull_request"]["number"].as_i64())?,
        head_sha: p["pull_request"]["head"]["sha"]
            .as_str()
            .map(str::to_string),
        head_ref: p["pull_request"]["head"]["ref"]
            .as_str()
            .map(str::to_string),
        url: sanitize_provider_url(p["pull_request"]["html_url"].as_str()),
        repo_url: p["repository"]["clone_url"]
            .as_str()
            .or_else(|| p["repository"]["html_url"].as_str())
            .map(str::to_string),
        gitlab_project_id: None,
    })
}

/// Whether two repository URLs name the same repository (scheme, case of
/// the host, a trailing slash and ".git" don't matter).
pub(crate) fn same_repo(a: &str, b: &str) -> bool {
    let norm = |u: &str| {
        let u = u.trim().trim_end_matches('/');
        let u = u.strip_suffix(".git").unwrap_or(u);
        let u = u
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        match u.split_once('/') {
            Some((host, path)) => format!("{}/{}", host.to_ascii_lowercase(), path),
            None => u.to_ascii_lowercase(),
        }
    };
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// Stop and expire every live preview of `app_id` for PR `number`; returns
/// the comment id of the newest one (to carry over) and how many there were.
async fn expire_pr_previews(state: &AppState, app_id: &str, number: i64) -> (Option<i64>, usize) {
    let existing: Vec<PreviewDeployment> = sqlx::query_as(
        "SELECT * FROM preview_deployments
         WHERE app_id = ? AND pr_number = ? AND status != 'expired'
         ORDER BY created_at",
    )
    .bind(app_id)
    .bind(number)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let carried = existing.iter().filter_map(|p| p.comment_id).next_back();
    for old in &existing {
        if let Some(ref cid) = old.container_id {
            let _ = docker::stop_container(&state.docker, cid).await;
            let _ = docker::remove_container(&state.docker, cid).await;
        }
        let _ = sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
            .bind(&old.id)
            .execute(&state.db)
            .await;
    }
    (carried, existing.len())
}

/// Replace the preview of `app` for `pr` and deploy it in the background.
async fn start_pr_preview(state: &AppState, app: &App, pr: &PrEvent) -> Result<(), &'static str> {
    let Some(sha) = pr.head_sha.clone() else {
        return Err("the event has no head commit");
    };
    if app.preview_enabled == 0 {
        return Err("previews are off for this app");
    }
    // Previews are served only under the preview domain (see domains.rs).
    let preview_domain = state.settings.read().await.preview_domain.clone();
    if preview_domain.is_empty() {
        return Err("no preview domain is configured");
    }
    preview_allowed(&state.db, &app.id).await?;

    let (carried_comment_id, _) = expire_pr_previews(state, &app.id, pr.number).await;

    let slug = tond_common::generate_raw_key()[..12].to_string();
    let ttl_seconds: i64 = 86400; // 24 h
    let now = chrono::Utc::now();
    let preview = PreviewDeployment {
        id: uuid::Uuid::new_v4().to_string(),
        app_id: app.id.clone(),
        subdomain: Some(preview_host(&slug, &app.name, &preview_domain)),
        slug,
        container_id: None,
        host_port: None,
        image_tag: None,
        ttl_seconds,
        status: "building".to_string(),
        expires_at: (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339(),
        created_at: now.to_rfc3339(),
        project_preview_id: None,
        pr_number: Some(pr.number),
        head_sha: Some(sha),
        head_ref: pr.head_ref.clone(),
        pr_url: pr.url.clone(),
        comment_id: carried_comment_id,
        gitlab_project_id: pr.gitlab_project_id,
    };
    sqlx::query(
        "INSERT INTO preview_deployments
         (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at,
          pr_number, head_sha, head_ref, pr_url, comment_id, gitlab_project_id)
         VALUES (?, ?, ?, ?, ?, 'building', ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&preview.id)
    .bind(&preview.app_id)
    .bind(&preview.slug)
    .bind(&preview.subdomain)
    .bind(preview.ttl_seconds)
    .bind(&preview.expires_at)
    .bind(&preview.created_at)
    .bind(preview.pr_number)
    .bind(&preview.head_sha)
    .bind(&preview.head_ref)
    .bind(&preview.pr_url)
    .bind(preview.comment_id)
    .bind(preview.gitlab_project_id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("[pr-preview/{}] couldn't store the preview: {e}", app.id);
        "couldn't store the preview"
    })?;

    crate::forge::announce_preview_building(state, app, &preview).await;

    let state = state.clone();
    let app = app.clone();
    let mut preview = preview;
    tokio::spawn(async move {
        if let Err(e) = deploy::deploy_preview(&state, &app, &mut preview).await {
            tracing::error!("[pr-preview/{}] preview deploy failed: {e:#}", app.name);
            let _ = sqlx::query("UPDATE preview_deployments SET status = 'failed' WHERE id = ?")
                .bind(&preview.id)
                .execute(&state.db)
                .await;
            crate::forge::announce_preview_failed(&state, &app, &preview, &format!("{e:#}")).await;
        }
        // On success deploy_preview() announces the preview itself.
    });
    Ok(())
}

/// Act on a pull/merge request event for one app.
async fn handle_pr_event(state: &AppState, app: &App, pr: &PrEvent, source: &str) -> Response {
    match &pr.action {
        PrAction::Deploy => match start_pr_preview(state, app, pr).await {
            Ok(()) => {
                tracing::info!("[{source}/{}] PR #{}: preview deploying", app.id, pr.number);
                (StatusCode::ACCEPTED, Json(ApiResponse::<()>::ok(()))).into_response()
            }
            Err(reason) if reason.contains("too many") || reason.contains("rate limit") => {
                tracing::warn!("[{source}/{}] {reason}", app.id);
                error_response(StatusCode::TOO_MANY_REQUESTS, reason)
            }
            Err(reason) => {
                tracing::debug!("[{source}/{}] PR #{} skipped: {reason}", app.id, pr.number);
                info_response(reason)
            }
        },
        PrAction::Close => {
            let (_, n) = expire_pr_previews(state, &app.id, pr.number).await;
            tracing::info!(
                "[{source}/{}] PR #{} closed: {n} preview(s) removed",
                app.id,
                pr.number
            );
            ok_response()
        }
        PrAction::Ignore(action) => {
            tracing::debug!("[{source}/{}] ignoring PR action {action}", app.id);
            ok_response()
        }
    }
}

/// A GitLab Merge Request Hook for the app of the route: previews for merge
/// requests of the app's own repository (commit statuses and the MR comment
/// need a GitLab token linked to the app).
async fn handle_gitlab_merge_request(
    state: &AppState,
    app: App,
    body: &Bytes,
) -> Result<Response, AppError> {
    let Some(pr) = parse_gitlab_merge_request(body) else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "invalid merge request payload",
        ));
    };
    if app.source_type != "git"
        || !pr
            .repo_url
            .as_deref()
            .is_some_and(|u| same_repo(u, app.source_url.as_deref().unwrap_or_default()))
    {
        tracing::debug!(
            "[webhook/gitlab-mr/{}] repository mismatch ({:?})",
            app.id,
            pr.repo_url
        );
        return Ok(info_response("merge request is for another repository"));
    }
    Ok(handle_pr_event(state, &app, &pr, "webhook/gitlab-mr").await)
}

/// A Gitea/Forgejo pull_request event for the app of the route (statuses and
/// the PR comment need a Gitea token linked to the app).
async fn handle_gitea_pull_request(
    state: &AppState,
    app: App,
    body: &Bytes,
) -> Result<Response, AppError> {
    let Some(pr) = parse_gitea_pull_request(body) else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "invalid pull request payload",
        ));
    };
    if app.source_type != "git"
        || !pr
            .repo_url
            .as_deref()
            .is_some_and(|u| same_repo(u, app.source_url.as_deref().unwrap_or_default()))
    {
        tracing::debug!(
            "[webhook/gitea-pr/{}] repository mismatch ({:?})",
            app.id,
            pr.repo_url
        );
        return Ok(info_response("pull request is for another repository"));
    }
    Ok(handle_pr_event(state, &app, &pr, "webhook/gitea-pr").await)
}
// ── GitHub App Events ──────────────────────────────────────────────────────
//
// POST /webhooks/source/github/events
//
// Receives webhook events from a registered GitHub App (push + pull_request),
// signed with that App's `webhook_secret` (github_apps table). The App is
// identified by the X-GitHub-Hook-Installation-Target-ID header (its numeric
// App ID); when that header is absent the signature is tried against each
// registered App. There is no "newest App" fallback: a request that does not
// verify against a registered App is rejected.
//
// Event routing:
//   push         → deploy the apps linked to this App whose source_url matches
//   pull_request → preview deployments for the linked apps
//   everything else → accepted without action

/// Find the registered GitHub App whose webhook secret signed this request.
async fn find_signing_github_app(
    db: &sqlx::SqlitePool,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Option<GitHubApp>, sqlx::Error> {
    let Some(signature) = header(headers, "x-hub-signature-256") else {
        return Ok(None);
    };

    let target_app_id: Option<i64> =
        header(headers, "x-github-hook-installation-target-id").and_then(|s| s.parse().ok());

    let candidates: Vec<GitHubApp> = match target_app_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM github_apps WHERE app_id = ?")
                .bind(id)
                .fetch_all(db)
                .await?
        }
        None => {
            sqlx::query_as("SELECT * FROM github_apps")
                .fetch_all(db)
                .await?
        }
    };

    Ok(candidates.into_iter().find(|gh_app| {
        secrets::github_webhook_secret(gh_app).is_ok_and(|secret| {
            !secret.is_empty() && verify_github_signature(signature, body, &secret)
        })
    }))
}

/// POST /webhooks/source/github/events
pub async fn github_app_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let event = header(&headers, "x-github-event").unwrap_or("").to_string();
    let delivery = header(&headers, "x-github-delivery").map(str::to_string);

    tracing::debug!(
        "[github-app-events] delivery={:?} event={}",
        delivery,
        event
    );

    let gh_app = match find_signing_github_app(&state.db, &headers, &body).await {
        Ok(Some(gh_app)) => gh_app,
        Ok(None) => {
            tracing::warn!(
                "[github-app-events] rejected: no registered App verifies the signature"
            );
            return error_response(
                StatusCode::UNAUTHORIZED,
                "invalid or missing webhook signature",
            );
        }
        Err(e) => {
            tracing::error!("[github-app-events] database error: {}", e);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };

    let scope = format!("github-app:{}", gh_app.id);
    match check_replay(&state.db, &scope, delivery.as_deref(), Some(&body)).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                "[github-app-events] rejected replayed delivery {:?}",
                delivery
            );
            return error_response(
                StatusCode::CONFLICT,
                "duplicate webhook delivery: this delivery was already received and is not processed twice. To deploy again, push a new commit or use Re-deploy in the Tond dashboard.",
            );
        }
        Err(e) => {
            tracing::error!("[github-app-events] database error: {}", e);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    }

    match event.as_str() {
        "push" => handle_github_app_push(&state, &gh_app, &body).await,
        "pull_request" => handle_github_app_pull_request(&state, &gh_app, &body).await,
        _ => ok_response(),
    }
}

/// Handle a push event from a GitHub App webhook: deploy every app linked to
/// this App whose source_url matches the pushed repository.
async fn handle_github_app_push(state: &AppState, gh_app: &GitHubApp, body: &Bytes) -> Response {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid JSON");
    };

    // Extract branch from "refs/heads/<branch>"
    let branch = payload["ref"]
        .as_str()
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .map(|s| s.to_string());

    // Extract the HTTPS clone URL of the pushed repo
    let repo_url = payload["repository"]["clone_url"]
        .as_str()
        .or_else(|| payload["repository"]["html_url"].as_str())
        .map(|s| s.to_string());

    let Some(repo_url) = repo_url else {
        tracing::warn!("[github-app-push] no repository.clone_url in payload");
        return ok_response();
    };

    let apps: Vec<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE source_url = ? AND source_type = 'git' AND github_app_id = ?",
    )
    .bind(&repo_url)
    .bind(&gh_app.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    if apps.is_empty() {
        tracing::debug!("[github-app-push] no apps matched repo {}", repo_url);
        return ok_response();
    }

    let mut triggered = 0usize;
    for app in apps {
        let resp = trigger_webhook_deploy(state, app, "github-app", branch.as_deref()).await;
        if resp.status() == StatusCode::ACCEPTED {
            triggered += 1;
        }
    }

    tracing::info!(
        "[github-app-push] triggered {} deploy(s) for repo {}",
        triggered,
        repo_url
    );

    (StatusCode::ACCEPTED, Json(ApiResponse::<()>::ok(()))).into_response()
}

// ── GitHub App PR Preview Deployments (Phase 4.6) ─────────────────────────
/// Handle a pull_request event from a GitHub App webhook: previews for every
/// app linked to this GitHub App that deploys the repository (see
/// "Pull/merge request previews" above). The App's signature only vouches
/// for apps that use this App.
async fn handle_github_app_pull_request(
    state: &AppState,
    gh_app: &GitHubApp,
    body: &Bytes,
) -> Response {
    let Some(pr) = parse_github_pull_request(body) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid pull request payload");
    };
    let Some(repo_url) = pr.repo_url.clone() else {
        tracing::warn!("[github-app-pr] no repository.clone_url in payload");
        return ok_response();
    };
    let apps: Vec<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE source_url = ? AND source_type = 'git' AND github_app_id = ?",
    )
    .bind(&repo_url)
    .bind(&gh_app.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if apps.is_empty() {
        tracing::debug!("[github-app-pr] no apps matched repo {}", repo_url);
        return ok_response();
    }
    for app in &apps {
        let _ = handle_pr_event(state, app, &pr, "github-app-pr").await;
    }
    ok_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{Bytes, to_bytes},
        extract::{Path, State},
        http::{HeaderMap, HeaderValue, StatusCode},
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    const PUSH_MAIN: &[u8] = br#"{"ref":"refs/heads/main","after":"abc"}"#;

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for webhook tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    /// Seed a git app. auto_deploy is off so a verified push never spawns a
    /// real deploy in tests; it answers "auto_deploy not enabled" instead.
    async fn seed_git_app(state: &AppState, name: &str) -> App {
        let app = App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://github.com/org/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );

        sqlx::query(
            "INSERT INTO apps (id, name, source_type, source_url, branch, container_port, status, webhook_secret, auto_deploy, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.source_type)
        .bind(&app.source_url)
        .bind(&app.branch)
        .bind(app.container_port)
        .bind(&app.status)
        .bind(
            crate::secrets::seal_webhook_secret(&secret_of(&app))
                .unwrap_or_else(|err| panic!("seal: {err}")),
        )
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed webhook app: {err}"));

        app
    }

    fn secret_of(app: &App) -> String {
        app.webhook_secret
            .clone()
            .unwrap_or_else(|| panic!("App::new must generate a webhook secret"))
    }

    fn github_signature(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .unwrap_or_else(|err| panic!("failed to initialize HMAC for test signature: {err}"));
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                *name,
                HeaderValue::from_str(value)
                    .unwrap_or_else(|err| panic!("invalid test header value: {err}")),
            );
        }
        map
    }

    async fn call_github(state: &AppState, app_id: &str, h: HeaderMap, body: &[u8]) -> Response {
        github_webhook(
            State(state.clone()),
            Path(app_id.to_string()),
            h,
            Bytes::copy_from_slice(body),
        )
        .await
        .unwrap_or_else(|err| panic!("github_webhook should return a response: {}", err.0))
    }

    // ── Pure helpers ──

    #[test]
    fn webhook_trigger_detail_names_the_provider_and_branch() {
        assert_eq!(
            webhook_trigger_detail("github", Some("main")),
            "GitHub push to main"
        );
        assert_eq!(webhook_trigger_detail("generic", None), "Generic webhook");
    }

    #[test]
    fn verify_github_signature_accepts_valid_signature_and_rejects_invalid_formats() {
        let body = br#"{"ref":"refs/heads/main"}"#;
        let secret = "topsecret";
        let signature = github_signature(secret, body);

        assert!(verify_github_signature(&signature, body, secret));
        assert!(!verify_github_signature(&signature, body, "other-secret"));
        assert!(!verify_github_signature("sha1=deadbeef", body, secret));
        assert!(!verify_github_signature("sha256=not-hex", body, secret));
        assert!(!verify_github_signature("", body, secret));
    }

    #[test]
    fn constant_time_eq_compares_contents_and_lengths() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-tokeN"));
        assert!(!constant_time_eq(b"secret", b"secret-token"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn verify_request_enforces_each_provider_scheme() {
        let secret = "s3cret-s3cret-s3cret";
        let body = br#"{"ref":"refs/heads/main"}"#;
        let sig = github_signature(secret, body);
        let hex_sig = sig.trim_start_matches("sha256=").to_string();

        // Nothing is accepted without credentials, for any provider.
        for provider in [
            Provider::GitHub,
            Provider::GitLab,
            Provider::Gitea,
            Provider::Generic,
        ] {
            assert!(
                !verify_request(provider, &HeaderMap::new(), body, secret),
                "{provider:?} must reject unsigned requests"
            );
            // An app without a secret rejects everything.
            assert!(!verify_request(
                provider,
                &headers(&[
                    ("x-hub-signature-256", &github_signature("", body)),
                    ("x-gitlab-token", ""),
                    ("authorization", "Bearer "),
                ]),
                body,
                ""
            ));
        }

        assert!(verify_request(
            Provider::GitHub,
            &headers(&[("x-hub-signature-256", &sig)]),
            body,
            secret
        ));
        assert!(verify_request(
            Provider::GitLab,
            &headers(&[("x-gitlab-token", secret)]),
            body,
            secret
        ));
        assert!(!verify_request(
            Provider::GitLab,
            &headers(&[("x-gitlab-token", "wrong")]),
            body,
            secret
        ));
        assert!(verify_request(
            Provider::Gitea,
            &headers(&[("x-gitea-signature", &hex_sig)]),
            body,
            secret
        ));
        assert!(verify_request(
            Provider::Gitea,
            &headers(&[("x-forgejo-signature", &hex_sig)]),
            body,
            secret
        ));
        assert!(!verify_request(
            Provider::Gitea,
            &headers(&[("x-gitea-signature", &hex_sig)]),
            b"tampered",
            secret
        ));
        assert!(verify_request(
            Provider::Generic,
            &headers(&[("x-tond-signature-256", &sig)]),
            body,
            secret
        ));
        assert!(verify_request(
            Provider::Generic,
            &headers(&[("authorization", &format!("Bearer {secret}"))]),
            body,
            secret
        ));
        assert!(!verify_request(
            Provider::Generic,
            &headers(&[("authorization", "Bearer not-the-secret")]),
            body,
            secret
        ));
    }

    #[test]
    fn delivery_ids_are_read_per_provider_and_bounded() {
        let long = "x".repeat(500);
        assert_eq!(
            delivery_id(Provider::GitHub, &headers(&[("x-github-delivery", "abc")])),
            Some("abc".to_string())
        );
        assert_eq!(
            delivery_id(
                Provider::GitLab,
                &headers(&[("x-gitlab-event-uuid", "evt"), ("idempotency-key", "idem")])
            ),
            Some("idem".to_string())
        );
        assert_eq!(
            delivery_id(Provider::Gitea, &headers(&[("x-forgejo-delivery", "f")])),
            Some("f".to_string())
        );
        assert_eq!(
            delivery_id(Provider::Generic, &headers(&[("x-tond-delivery", &long)]))
                .map(|id| id.len()),
            Some(128)
        );
        assert_eq!(delivery_id(Provider::GitHub, &HeaderMap::new()), None);
    }

    #[test]
    fn push_branch_extractor_parses_refs_and_handles_invalid_payloads() {
        assert_eq!(
            extract_push_branch(br#"{"ref":"refs/heads/feature/login"}"#),
            Some("feature/login".to_string())
        );
        assert_eq!(extract_push_branch(br#"{"unexpected":true}"#), None);
        assert_eq!(extract_push_branch(b"not json"), None);
    }

    #[test]
    fn sanitize_provider_url_only_accepts_plain_https_links() {
        assert_eq!(
            sanitize_provider_url(Some("https://github.com/org/repo/pull/7")),
            Some("https://github.com/org/repo/pull/7".to_string())
        );
        assert_eq!(sanitize_provider_url(Some("javascript:alert(1)")), None);
        assert_eq!(
            sanitize_provider_url(Some("JavaScript://github.com/%0aalert(1)")),
            None
        );
        assert_eq!(sanitize_provider_url(Some("data:text/html,hi")), None);
        assert_eq!(sanitize_provider_url(Some("http://github.com/x")), None);
        assert_eq!(
            sanitize_provider_url(Some("https://user:pass@github.com/x")),
            None
        );
        assert_eq!(sanitize_provider_url(Some("not a url")), None);
        assert_eq!(sanitize_provider_url(None), None);
    }

    // ── Replay protection and limits ──

    #[tokio::test]
    async fn remember_delivery_rejects_repeats_and_prunes_expired_rows() {
        let state = test_state().await;
        let old =
            (chrono::Utc::now() - chrono::Duration::hours(REPLAY_WINDOW_HOURS + 1)).to_rfc3339();
        sqlx::query("INSERT INTO webhook_deliveries (key, received_at) VALUES ('stale', ?)")
            .bind(&old)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to seed stale delivery: {err}"));

        assert!(matches!(remember_delivery(&state.db, "k1").await, Ok(true)));
        assert!(matches!(
            remember_delivery(&state.db, "k1").await,
            Ok(false)
        ));

        let stale: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE key = 'stale'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to count deliveries: {err}"));
        assert_eq!(stale, 0, "expired deliveries should be pruned");

        // An expired key can be used again.
        assert!(matches!(
            remember_delivery(&state.db, "stale").await,
            Ok(true)
        ));
    }

    #[tokio::test]
    async fn webhook_deploy_limiter_caps_bursts_per_app() {
        let app_id = uuid::Uuid::new_v4().to_string();
        for _ in 0..WEBHOOK_BURST {
            assert!(take_deploy_token(&app_id).await);
        }
        assert!(
            !take_deploy_token(&app_id).await,
            "the burst after {WEBHOOK_BURST} webhook deploys should be refused"
        );
        assert!(
            take_deploy_token(&uuid::Uuid::new_v4().to_string()).await,
            "limits are per app"
        );
    }

    #[tokio::test]
    async fn preview_allowed_caps_active_previews_per_app() {
        let state = test_state().await;
        let app = seed_git_app(&state, "previews").await;
        assert!(preview_allowed(&state.db, &app.id).await.is_ok());

        for i in 0..MAX_ACTIVE_PREVIEWS_PER_APP {
            sqlx::query(
                "INSERT INTO preview_deployments (id, app_id, slug, status, expires_at) VALUES (?, ?, ?, 'running', ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&app.id)
            .bind(format!("slug{i}"))
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to seed preview: {err}"));
        }

        assert_eq!(
            preview_allowed(&state.db, &app.id).await,
            Err("too many active previews for this app")
        );
    }

    // ── Per-app endpoints ──

    #[tokio::test]
    async fn app_creation_generates_unique_webhook_secrets() {
        let a = App::new("a".into(), "git".into(), None, None, 3000);
        let b = App::new("b".into(), "git".into(), None, None, 3000);
        assert_eq!(secret_of(&a).len(), 64);
        assert_ne!(secret_of(&a), secret_of(&b));
    }

    #[tokio::test]
    async fn github_webhook_rejects_unsigned_and_invalid_requests() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;

        let unsigned = call_github(
            &state,
            &app.id,
            headers(&[("x-github-event", "push")]),
            PUSH_MAIN,
        )
        .await;
        assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

        let bad = call_github(
            &state,
            &app.id,
            headers(&[
                ("x-github-event", "push"),
                ("x-hub-signature-256", "sha256=deadbeef"),
            ]),
            PUSH_MAIN,
        )
        .await;
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(bad).await;
        assert_eq!(
            body["error"]["message"],
            "invalid or missing webhook signature"
        );

        // Even a ping must be signed.
        let ping = call_github(
            &state,
            &app.id,
            headers(&[("x-github-event", "ping")]),
            b"{}",
        )
        .await;
        assert_eq!(ping.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn github_webhook_accepts_signed_push_and_rejects_replays() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        let sig = github_signature(&secret_of(&app), PUSH_MAIN);

        let first = call_github(
            &state,
            &app.id,
            headers(&[
                ("x-github-event", "push"),
                ("x-github-delivery", "d-1"),
                ("x-hub-signature-256", &sig),
            ]),
            PUSH_MAIN,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            response_json(first).await["message"],
            "auto_deploy not enabled"
        );

        // Same body under a new delivery ID: the body digest catches it.
        let same_body = call_github(
            &state,
            &app.id,
            headers(&[
                ("x-github-event", "push"),
                ("x-github-delivery", "d-2"),
                ("x-hub-signature-256", &sig),
            ]),
            PUSH_MAIN,
        )
        .await;
        assert_eq!(same_body.status(), StatusCode::CONFLICT);

        // New body, reused delivery ID: the delivery ID catches it.
        let other_body = br#"{"ref":"refs/heads/main","after":"def"}"#;
        let reused_id = call_github(
            &state,
            &app.id,
            headers(&[
                ("x-github-event", "push"),
                ("x-github-delivery", "d-1"),
                (
                    "x-hub-signature-256",
                    &github_signature(&secret_of(&app), other_body),
                ),
            ]),
            other_body,
        )
        .await;
        assert_eq!(reused_id.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn github_webhook_returns_branch_not_watched_for_mismatched_branch() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET auto_deploy = 1, watch_branch = 'main' WHERE id = ?")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to set watch_branch: {err}"));
        let body = br#"{"ref":"refs/heads/feature-branch"}"#;

        let response = call_github(
            &state,
            &app.id,
            headers(&[
                ("x-github-event", "push"),
                (
                    "x-hub-signature-256",
                    &github_signature(&secret_of(&app), body),
                ),
            ]),
            body,
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["message"],
            "branch not watched"
        );
    }

    #[tokio::test]
    async fn webhooks_are_addressed_by_uuid_not_name() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        let result = github_webhook(
            State(state.clone()),
            Path(app.name.clone()),
            headers(&[(
                "x-hub-signature-256",
                &github_signature(&secret_of(&app), PUSH_MAIN),
            )]),
            Bytes::from_static(PUSH_MAIN),
        )
        .await;
        assert!(
            matches!(result, Err(AppError(TondError::AppNotFound(_)))),
            "an app name must not resolve to a webhook endpoint"
        );
    }

    #[tokio::test]
    async fn gitlab_webhook_requires_the_secret_token() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;

        for token in [None, Some("wrong-token")] {
            let mut h = headers(&[("x-gitlab-event", "Push Hook")]);
            if let Some(t) = token {
                h.insert("x-gitlab-token", HeaderValue::from_static(t));
            }
            let response = gitlab_webhook(
                State(state.clone()),
                Path(app.id.clone()),
                h,
                Bytes::from_static(PUSH_MAIN),
            )
            .await
            .unwrap_or_else(|err| panic!("gitlab_webhook should return a response: {}", err.0));
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let response = gitlab_webhook(
            State(state.clone()),
            Path(app.id.clone()),
            headers(&[
                ("x-gitlab-event", "Push Hook"),
                ("x-gitlab-token", &secret_of(&app)),
            ]),
            Bytes::from_static(PUSH_MAIN),
        )
        .await
        .unwrap_or_else(|err| panic!("gitlab_webhook should return a response: {}", err.0));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["message"],
            "auto_deploy not enabled"
        );
    }

    #[tokio::test]
    async fn gitea_webhook_requires_a_valid_signature() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        let sig = github_signature(&secret_of(&app), PUSH_MAIN);
        let hex_sig = sig.trim_start_matches("sha256=");

        let unsigned = gitea_webhook(
            State(state.clone()),
            Path(app.id.clone()),
            headers(&[("x-gitea-event", "push")]),
            Bytes::from_static(PUSH_MAIN),
        )
        .await
        .unwrap_or_else(|err| panic!("gitea_webhook should return a response: {}", err.0));
        assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

        let signed = gitea_webhook(
            State(state.clone()),
            Path(app.id.clone()),
            headers(&[
                ("x-gitea-event", "push"),
                ("x-gitea-delivery", "g-1"),
                ("x-gitea-signature", hex_sig),
            ]),
            Bytes::from_static(PUSH_MAIN),
        )
        .await
        .unwrap_or_else(|err| panic!("gitea_webhook should return a response: {}", err.0));
        assert_eq!(signed.status(), StatusCode::OK);
        assert_eq!(
            response_json(signed).await["message"],
            "auto_deploy not enabled"
        );
    }

    fn gitea_pr(action: &str, repo: &str) -> Vec<u8> {
        serde_json::json!({
            "action": action,
            "number": 7,
            "pull_request": {
                "number": 7,
                "html_url": "https://codeberg.org/org/repo/pulls/7",
                "head": { "sha": "0123456789abcdef", "ref": "feature" }
            },
            "repository": { "clone_url": repo }
        })
        .to_string()
        .into_bytes()
    }

    async fn send_gitea(
        state: &AppState,
        app: &App,
        event: &str,
        body: Vec<u8>,
        delivery: &str,
    ) -> Response {
        let sig = github_signature(&secret_of(app), &body);
        gitea_webhook(
            State(state.clone()),
            Path(app.id.clone()),
            headers(&[
                ("x-gitea-event", event),
                ("x-gitea-delivery", delivery),
                ("x-gitea-signature", sig.trim_start_matches("sha256=")),
            ]),
            Bytes::from(body),
        )
        .await
        .unwrap_or_else(|err| panic!("gitea_webhook should return a response: {}", err.0))
    }

    #[test]
    fn pull_request_events_of_every_forge_reduce_to_the_same_shape() {
        let gitea = parse_gitea_pull_request(&gitea_pr(
            "synchronized",
            "https://codeberg.org/org/repo.git",
        ))
        .unwrap_or_else(|| panic!("gitea event parses"));
        assert_eq!(gitea.action, PrAction::Deploy);
        assert_eq!(
            (gitea.number, gitea.head_sha.as_deref()),
            (7, Some("0123456789abcdef"))
        );
        assert_eq!(gitea.head_ref.as_deref(), Some("feature"));
        assert_eq!(
            parse_gitea_pull_request(&gitea_pr("closed", "x")).map(|p| p.action),
            Some(PrAction::Close)
        );
        assert_eq!(
            parse_gitea_pull_request(&gitea_pr("label_updated", "x")).map(|p| p.action),
            Some(PrAction::Ignore("label_updated".to_string()))
        );

        let gitlab = parse_gitlab_merge_request(
            br#"{"object_attributes":{"action":"merge","iid":3,"url":"https://gitlab.com/g/p/-/merge_requests/3","last_commit":{"id":"abc"},"source_branch":"f"},"project":{"id":42,"http_url_to_repo":"https://gitlab.com/g/p.git"}}"#,
        )
        .unwrap_or_else(|| panic!("gitlab event parses"));
        assert_eq!(gitlab.action, PrAction::Close);
        assert_eq!((gitlab.number, gitlab.gitlab_project_id), (3, Some(42)));

        let github = parse_github_pull_request(
            br#"{"action":"reopened","number":9,"pull_request":{"head":{"sha":"s","ref":"r"},"html_url":"https://github.com/o/r/pull/9"},"repository":{"clone_url":"https://github.com/o/r.git"}}"#,
        )
        .unwrap_or_else(|| panic!("github event parses"));
        assert_eq!(github.action, PrAction::Deploy);
        assert_eq!(github.url.as_deref(), Some("https://github.com/o/r/pull/9"));
        assert!(parse_gitea_pull_request(b"not json").is_none());
    }

    #[test]
    fn repositories_match_regardless_of_scheme_case_and_suffix() {
        assert!(same_repo(
            "https://GitHub.com/org/repo.git",
            "http://github.com/org/repo/"
        ));
        assert!(!same_repo(
            "https://github.com/org/repo",
            "https://github.com/org/other"
        ));
        assert!(!same_repo(
            "https://github.com/Org/repo",
            "https://github.com/org/repo"
        ));
        assert!(!same_repo("", ""));
    }

    #[tokio::test]
    async fn gitea_pull_requests_drive_previews_of_their_own_repository_only() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await; // source: github.com/org/repo

        // Another repository: ignored.
        let other = send_gitea(
            &state,
            &app,
            "pull_request",
            gitea_pr("opened", "https://codeberg.org/x/y.git"),
            "g-1",
        )
        .await;
        assert_eq!(other.status(), StatusCode::OK);
        assert_eq!(
            response_json(other).await["message"],
            "pull request is for another repository"
        );

        // Its repository, previews off: nothing deployed.
        let off = send_gitea(
            &state,
            &app,
            "pull_request",
            gitea_pr("opened", "https://github.com/org/repo.git"),
            "g-2",
        )
        .await;
        assert_eq!(
            response_json(off).await["message"],
            "previews are off for this app"
        );

        // Closing removes the PR's live previews (and only those).
        for (id, pr, status) in [
            ("p1", 7, "running"),
            ("p2", 7, "failed"),
            ("p3", 8, "running"),
        ] {
            sqlx::query(
                "INSERT INTO preview_deployments (id, app_id, slug, status, expires_at, pr_number) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&app.id)
            .bind(id)
            .bind(status)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(pr)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("seed preview: {err}"));
        }
        let closed = send_gitea(
            &state,
            &app,
            "pull_request",
            gitea_pr("closed", "https://github.com/org/repo"),
            "g-3",
        )
        .await;
        assert_eq!(closed.status(), StatusCode::OK);
        let live: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM preview_deployments WHERE status != 'expired' ORDER BY id",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(live, vec!["p3"]);
    }

    #[tokio::test]
    async fn generic_webhook_requires_the_app_secret() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;

        for h in [
            HeaderMap::new(),
            headers(&[("authorization", "Bearer invalid-key")]),
        ] {
            let response =
                generic_webhook(State(state.clone()), Path(app.id.clone()), h, Bytes::new())
                    .await
                    .unwrap_or_else(|err| {
                        panic!("generic_webhook should return a response: {}", err.0)
                    });
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        // Repeated identical CI calls are fine without a delivery header...
        for _ in 0..2 {
            let response = generic_webhook(
                State(state.clone()),
                Path(app.id.clone()),
                headers(&[("authorization", &format!("Bearer {}", secret_of(&app)))]),
                Bytes::new(),
            )
            .await
            .unwrap_or_else(|err| panic!("generic_webhook should return a response: {}", err.0));
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response_json(response).await["message"],
                "auto_deploy not enabled"
            );
        }

        // ...but a repeated X-Tond-Delivery is a replay.
        let mut statuses = Vec::new();
        for _ in 0..2 {
            let response = generic_webhook(
                State(state.clone()),
                Path(app.id.clone()),
                headers(&[
                    ("authorization", &format!("Bearer {}", secret_of(&app))),
                    ("x-tond-delivery", "ci-run-42"),
                ]),
                Bytes::new(),
            )
            .await
            .unwrap_or_else(|err| panic!("generic_webhook should return a response: {}", err.0));
            statuses.push(response.status());
        }
        assert_eq!(statuses, vec![StatusCode::OK, StatusCode::CONFLICT]);
    }

    // ── GitHub App events ──

    async fn seed_github_app(state: &AppState, id: &str, app_id: i64, secret: &str) {
        let sealed = crate::secrets::seal_github_credentials("csecret", secret, "pem")
            .unwrap_or_else(|err| panic!("seal: {err}"));
        sqlx::query(
            "INSERT INTO github_apps (id, name, app_id, client_id, client_secret, webhook_secret, private_key, slug)
             VALUES (?, 'gh', ?, 'cid', ?, ?, ?, 'slug')",
        )
        .bind(id)
        .bind(app_id)
        .bind(sealed.0)
        .bind(sealed.1)
        .bind(sealed.2)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed GitHub App: {err}"));
    }

    async fn call_app_events(state: &AppState, h: HeaderMap, body: &[u8]) -> StatusCode {
        github_app_events(State(state.clone()), h, Bytes::copy_from_slice(body))
            .await
            .status()
    }

    #[tokio::test]
    async fn github_app_events_require_a_registered_apps_signature() {
        let state = test_state().await;
        let body = br#"{"zen":"hi"}"#;

        // No App registered: nothing verifies, so the request is rejected
        // (previously it was silently accepted).
        assert_eq!(
            call_app_events(
                &state,
                headers(&[
                    ("x-github-event", "ping"),
                    ("x-hub-signature-256", &github_signature("x", body))
                ]),
                body
            )
            .await,
            StatusCode::UNAUTHORIZED
        );

        seed_github_app(&state, "gh-old", 1, "old-secret").await;
        seed_github_app(&state, "gh-new", 2, "new-secret").await;

        // Unsigned and wrongly signed requests are rejected.
        assert_eq!(
            call_app_events(&state, headers(&[("x-github-event", "ping")]), body).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call_app_events(
                &state,
                headers(&[
                    ("x-github-event", "ping"),
                    ("x-hub-signature-256", &github_signature("attacker", body))
                ]),
                body
            )
            .await,
            StatusCode::UNAUTHORIZED
        );

        // The target-ID header selects the App; a signature from another App fails.
        assert_eq!(
            call_app_events(
                &state,
                headers(&[
                    ("x-github-event", "ping"),
                    ("x-github-hook-installation-target-id", "2"),
                    ("x-hub-signature-256", &github_signature("old-secret", body))
                ]),
                body
            )
            .await,
            StatusCode::UNAUTHORIZED
        );

        // Without the header, the older App's signature is still found (no
        // "newest row" assumption).
        assert_eq!(
            call_app_events(
                &state,
                headers(&[
                    ("x-github-event", "ping"),
                    ("x-github-delivery", "gh-d-1"),
                    ("x-hub-signature-256", &github_signature("old-secret", body))
                ]),
                body
            )
            .await,
            StatusCode::OK
        );

        // And the same delivery again is a replay.
        assert_eq!(
            call_app_events(
                &state,
                headers(&[
                    ("x-github-event", "ping"),
                    ("x-github-delivery", "gh-d-1"),
                    ("x-hub-signature-256", &github_signature("old-secret", body))
                ]),
                body
            )
            .await,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn github_app_push_only_targets_apps_linked_to_that_app() {
        let state = test_state().await;
        seed_github_app(&state, "gh-1", 1, "gh-secret").await;
        // Linked app with auto_deploy on but a watch branch that does not match,
        // so nothing is actually deployed; an unlinked app with the same repo.
        let linked = seed_git_app(&state, "linked").await;
        let unlinked = seed_git_app(&state, "unlinked").await;
        sqlx::query(
            "UPDATE apps SET github_app_id = 'gh-1', auto_deploy = 1, watch_branch = 'release' WHERE id = ?",
        )
        .bind(&linked.id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to link app: {err}"));
        sqlx::query("UPDATE apps SET auto_deploy = 1 WHERE id = ?")
            .bind(&unlinked.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to update app: {err}"));

        let body = br#"{"ref":"refs/heads/main","repository":{"clone_url":"https://github.com/org/repo.git"}}"#;
        let status = call_app_events(
            &state,
            headers(&[
                ("x-github-event", "push"),
                ("x-hub-signature-256", &github_signature("gh-secret", body)),
            ]),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);

        // Had the unlinked app been matched, its deploy would have consumed a
        // webhook token; its full burst must still be available.
        for _ in 0..WEBHOOK_BURST {
            assert!(
                take_deploy_token(&unlinked.id).await,
                "the unlinked app must not be deployed by another GitHub App's webhook"
            );
        }
    }
}
