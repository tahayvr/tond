// handlers/webhooks.rs — Webhook endpoints for GitHub, GitLab, and Generic providers
//
// Three webhook endpoints, all under /api/webhooks/{provider}/{app_id}:
//
//   POST /api/webhooks/github/{app_id}
//     - Validates the X-Hub-Signature-256 HMAC header (if webhook_secret is set)
//     - Parses the X-GitHub-Event header; only "push" events trigger a deploy
//     - Checks that the pushed ref matches the app's watch_branch (if set)
//     - Only deploys when auto_deploy = true on the app
//
//   POST /api/webhooks/gitlab/{app_id}
//     - Validates the X-Gitlab-Token header (if webhook_secret is set)
//     - Parses event type from X-Gitlab-Event; only "Push Hook" triggers a deploy
//     - Same branch filter and auto_deploy guard as GitHub
//
//   POST /api/webhooks/generic/{app_id}
//     - Any POST triggers a deploy when auto_deploy = true
//     - Requires a valid Tond API key in the Authorization: Bearer header.
//       The key is verified against the api_keys table (same as RequireAuth).
//       Returns 401 if the header is missing or the key is invalid.
//
// Security notes:
//   - Signature validation uses a constant-time comparison (subtle crate via
//     hmac::Mac::verify_slice) to prevent timing attacks.
//   - GitHub/GitLab webhook endpoints are public (no RequireAuth extractor) —
//     they must be callable by external services. The HMAC/token secret and
//     auto_deploy flag together gate the actual deploy.
//   - The generic webhook requires a Tond API key (Bearer token) to prevent
//     anyone who discovers the URL from triggering arbitrary deploys.
//   - If auto_deploy = false, all webhooks return 200 but do nothing.
//
// Deploy trigger:
//   All three handlers call the shared trigger_webhook_deploy() helper which:
//     1. Fetches the app from DB (404 if not found)
//     2. Checks auto_deploy flag (200 + "auto_deploy not enabled" if false)
//     3. Spawns a background deploy task (same as POST /api/apps/{id}/deploy)
//     4. Returns 202 Accepted

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tond_common::{ApiResponse, App, GitHubApp, GitLabToken, PreviewDeployment};

use crate::{
    alerts, auth, deploy, docker,
    domains::preview_host,
    error::AppError,
    handlers::{github_apps, gitlab_tokens},
    state::AppState,
};

// ── Shared deploy trigger ──────────────────────────────────────────────────

/// Fetch the app, check auto_deploy, and spawn a background deploy.
/// Returns a concrete Response to avoid Rust 2024 impl-Trait lifetime issues.
async fn trigger_webhook_deploy(
    state: &AppState,
    app_id: &str,
    provider: &str,
    branch: Option<&str>,
) -> Result<Response, AppError> {
    let mut app = fetch_app_for_webhook(&state.db, app_id).await?;

    if app.auto_deploy == 0 {
        tracing::debug!(
            "[webhook/{}/{}] auto_deploy not enabled — skipping",
            provider,
            app_id
        );
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::<()>::ok_info("auto_deploy not enabled")),
        )
            .into_response());
    }

    // Branch filter: if watch_branch is set, only deploy when the pushed branch matches.
    if let Some(watch) = &app.watch_branch
        && let Some(pushed) = branch
        && pushed != watch
    {
        tracing::debug!(
            "[webhook/{}/{}] branch '{}' != watch_branch '{}' — skipping",
            provider,
            app_id,
            pushed,
            watch
        );
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::<()>::ok_info("branch not watched")),
        )
            .into_response());
    }

    tracing::info!(
        "[webhook/{}/{}] triggering deploy (branch: {:?})",
        provider,
        app_id,
        branch
    );

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = deploy::deploy(&state_clone, &mut app, "webhook").await {
            tracing::error!("[webhook] deploy failed for {}: {}", app.id, e);
            let _ = sqlx::query(
                "UPDATE apps SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
            )
            .bind(&app.id)
            .execute(&state_clone.db)
            .await;
            alerts::fire_event_alerts(
                &state_clone,
                "deployment_failure",
                Some(app.id.as_str()),
                Some(app.name.as_str()),
                Some(&e.to_string()),
            )
            .await;
        } else {
            alerts::fire_event_alerts(
                &state_clone,
                "deployment_success",
                Some(app.id.as_str()),
                Some(app.name.as_str()),
                None,
            )
            .await;
        }
    });

    Ok((StatusCode::ACCEPTED, Json(ApiResponse::<()>::ok(()))).into_response())
}

/// Fetch an app by id or name for webhook use. Returns a plain AppError (404) if not found.
async fn fetch_app_for_webhook(db: &sqlx::SqlitePool, id_or_name: &str) -> Result<App, AppError> {
    use tond_common::TondError;

    let app = sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(db)
        .await?;

    app.ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.to_string())))
}

// ── GitHub ─────────────────────────────────────────────────────────────────

/// POST /api/webhooks/github/{app_id}
///
/// Validates X-Hub-Signature-256 (if webhook_secret set), filters to "push"
/// events, extracts the branch from `ref`, then triggers a deploy.
pub async fn github_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // ── Fetch app for secret validation ──
    let app = fetch_app_for_webhook(&state.db, &app_id).await?;

    // ── Validate HMAC-SHA256 signature ──
    if let Some(ref secret) = app.webhook_secret {
        let sig_header = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !verify_github_signature(sig_header, &body, secret) {
            return Ok((
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("invalid signature")),
            )
                .into_response());
        }
    }

    // ── Event type filter ──
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if event != "push" {
        // Non-push events (e.g. ping, pull_request) are silently accepted.
        return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
    }

    // ── Extract branch from payload ──
    let branch = extract_github_branch(&body);

    trigger_webhook_deploy(&state, &app_id, "github", branch.as_deref()).await
}

/// Validate a GitHub X-Hub-Signature-256 header.
/// Format: "sha256=<hex_digest>"
fn verify_github_signature(header: &str, body: &[u8], secret: &str) -> bool {
    let Some(hex_digest) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected) = hex::decode(hex_digest) else {
        return false;
    };

    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// Extract the short branch name from a GitHub push payload.
/// `ref` field is "refs/heads/<branch>".
fn extract_github_branch(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let r = v.get("ref")?.as_str()?;
    // Strip "refs/heads/" prefix
    Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
}

// ── GitLab ─────────────────────────────────────────────────────────────────

/// POST /api/webhooks/gitlab/{app_id}
///
/// Validates X-Gitlab-Token (if webhook_secret set), filters to "Push Hook"
/// events, extracts the branch, then triggers a deploy.
pub async fn gitlab_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let app = fetch_app_for_webhook(&state.db, &app_id).await?;

    // ── Validate secret token ──
    if let Some(ref secret) = app.webhook_secret {
        let token = headers
            .get("x-gitlab-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if token != secret {
            return Ok((
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("invalid token")),
            )
                .into_response());
        }
    }

    // ── Event type filter ──
    let event = headers
        .get("x-gitlab-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if event != "Push Hook" {
        if event == "Merge Request Hook" {
            return handle_gitlab_merge_request(&state, &app_id, &body).await;
        }
        // All other GitLab events are silently accepted.
        return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
    }

    // ── Extract branch from payload ──
    let branch = extract_gitlab_branch(&body);

    trigger_webhook_deploy(&state, &app_id, "gitlab", branch.as_deref()).await
}

/// Handle a Merge Request Hook event from a GitLab webhook.
///
/// Actions handled:
///   "open" | "update" | "reopen"
///     – Find apps whose `source_url` matches the MR's repo clone URL and
///       that are linked to a GitLab token (`gitlab_token_id IS NOT NULL`).
///     – Expire any existing preview deployments for this app+MR.
///     – Insert a new `preview_deployments` row (status = "building") with
///       MR metadata (`pr_number` = MR IID, `head_sha`, `head_ref`, `pr_url`).
///     – Post a "pending" GitLab commit status (best-effort).
///     – Spawn `deploy_preview()` in the background.
///
///   "close" | "merge"
///     – Find and expire all active previews for this app+MR.
async fn handle_gitlab_merge_request(
    state: &AppState,
    app_id: &str,
    body: &Bytes,
) -> Result<Response, AppError> {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("invalid JSON")),
        )
            .into_response());
    };

    let action = payload["object_attributes"]["action"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mr_iid = payload["object_attributes"]["iid"].as_i64().unwrap_or(0);
    let head_sha = payload["object_attributes"]["last_commit"]["id"]
        .as_str()
        .map(|s| s.to_string());
    let head_ref = payload["object_attributes"]["source_branch"]
        .as_str()
        .map(|s| s.to_string());
    let mr_url = payload["object_attributes"]["url"]
        .as_str()
        .map(|s| s.to_string());
    let repo_http_url = payload["project"]["http_url_to_repo"]
        .as_str()
        .or_else(|| payload["repository"]["homepage"].as_str())
        .map(|s| s.to_string());
    let gitlab_project_id = payload["project"]["id"].as_i64().unwrap_or(0);

    let Some(repo_url) = repo_http_url else {
        tracing::warn!(
            "[webhook/gitlab-mr/{}] no project.http_url_to_repo in payload",
            app_id
        );
        return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
    };

    // Find the specific app (filtered by app_id from the route) + ensure it
    // matches the incoming repo URL.
    let app: Option<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE id = ? AND source_url = ? AND source_type = 'git'",
    )
    .bind(app_id)
    .bind(&repo_url)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let Some(app) = app else {
        tracing::debug!(
            "[webhook/gitlab-mr/{}] app not found or repo URL mismatch ({})",
            app_id,
            repo_url
        );
        return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
    };

    // Fetch the associated GitLab token (needed for commit status API calls).
    let gl_token: Option<GitLabToken> = if let Some(ref tid) = app.gitlab_token_id {
        sqlx::query_as("SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1")
            .bind(tid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
    } else {
        None
    };

    // Previews are served only under the preview domain (see domains.rs).
    let preview_domain: Option<String> = {
        let s = state.settings.read().await;
        Some(s.preview_domain.clone()).filter(|d| !d.is_empty())
    };

    match action.as_str() {
        "open" | "update" | "reopen" => {
            let Some(ref sha) = head_sha else {
                tracing::warn!(
                    "[webhook/gitlab-mr/{}] missing last_commit.id in payload",
                    app_id
                );
                return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
            };

            if app.preview_enabled == 0 {
                tracing::debug!(
                    "[webhook/gitlab-mr/{}] skipping — preview_enabled=false",
                    app_id
                );
                return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
            }
            let Some(ref preview_domain) = preview_domain else {
                tracing::warn!(
                    "[webhook/gitlab-mr/{}] skipping — no preview domain configured",
                    app_id
                );
                return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
            };

            // Expire any existing previews for this app+MR (best-effort).
            let existing: Vec<PreviewDeployment> = sqlx::query_as(
                "SELECT * FROM preview_deployments
                 WHERE app_id = ? AND pr_number = ? AND status != 'expired'",
            )
            .bind(&app.id)
            .bind(mr_iid)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            for old in existing {
                if let Some(ref cid) = old.container_id {
                    let _ = docker::stop_container(&state.docker, cid).await;
                    let _ = docker::remove_container(&state.docker, cid).await;
                }
                let _ =
                    sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
                        .bind(&old.id)
                        .execute(&state.db)
                        .await;
            }

            // Build the new preview row.
            let slug = {
                use tond_common::generate_raw_key;
                generate_raw_key()[..12].to_string()
            };
            let ttl_seconds: i64 = 86400; // 24 h
            let now = chrono::Utc::now();
            let expires_at = (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339();
            let created_at = now.to_rfc3339();
            let subdomain = preview_host(&slug, &app.name, preview_domain);
            let preview_id = uuid::Uuid::new_v4().to_string();

            let insert = sqlx::query(
                "INSERT INTO preview_deployments
                 (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at,
                  pr_number, head_sha, head_ref, pr_url, gitlab_project_id)
                 VALUES (?, ?, ?, ?, ?, 'building', ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&preview_id)
            .bind(&app.id)
            .bind(&slug)
            .bind(&subdomain)
            .bind(ttl_seconds)
            .bind(&expires_at)
            .bind(&created_at)
            .bind(mr_iid)
            .bind(sha)
            .bind(head_ref.as_deref())
            .bind(mr_url.as_deref())
            .bind(if gitlab_project_id > 0 {
                Some(gitlab_project_id)
            } else {
                None
            })
            .execute(&state.db)
            .await;

            if let Err(e) = insert {
                tracing::error!(
                    "[webhook/gitlab-mr/{}] failed to insert preview row: {}",
                    app_id,
                    e
                );
                return Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response());
            }

            // Post "pending" commit status (best-effort).
            if let Some(ref token) = gl_token
                && gitlab_project_id > 0
            {
                let _ = gitlab_tokens::post_gitlab_commit_status(
                    token,
                    gitlab_project_id,
                    sha,
                    "pending",
                    "Building preview environment…",
                    None,
                )
                .await
                .map_err(|e| {
                    tracing::warn!(
                        "[webhook/gitlab-mr/{}] failed to post pending status: {}",
                        app_id,
                        e
                    );
                });
            }

            // Fetch the just-inserted row and spawn deploy.
            let preview_row = sqlx::query_as::<_, PreviewDeployment>(
                "SELECT * FROM preview_deployments WHERE id = ?",
            )
            .bind(&preview_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            let mut preview = match preview_row {
                Some(p) => p,
                None => PreviewDeployment {
                    id: preview_id.clone(),
                    app_id: app.id.clone(),
                    slug: slug.clone(),
                    subdomain: Some(subdomain.clone()),
                    container_id: None,
                    host_port: None,
                    image_tag: None,
                    ttl_seconds,
                    status: "building".to_string(),
                    expires_at: expires_at.clone(),
                    created_at: created_at.clone(),
                    project_preview_id: None,
                    pr_number: Some(mr_iid),
                    head_sha: Some(sha.clone()),
                    head_ref: head_ref.clone(),
                    pr_url: mr_url.clone(),
                    comment_id: None,
                    gitlab_project_id: if gitlab_project_id > 0 {
                        Some(gitlab_project_id)
                    } else {
                        None
                    },
                },
            };

            let state_clone = state.clone();
            let app_clone = app.clone();
            let gl_token_clone = gl_token.clone();
            let sha_clone = sha.clone();
            let app_id_clone = app_id.to_string();

            tokio::spawn(async move {
                if let Err(e) = deploy::deploy_preview(&state_clone, &app_clone, &mut preview).await
                {
                    tracing::error!(
                        "[webhook/gitlab-mr/{}] preview deploy failed: {}",
                        app_id_clone,
                        e
                    );
                    let _ = sqlx::query(
                        "UPDATE preview_deployments SET status = 'failed' WHERE id = ?",
                    )
                    .bind(&preview.id)
                    .execute(&state_clone.db)
                    .await;

                    // Post "failed" commit status.
                    if let (Some(token), true) = (&gl_token_clone, gitlab_project_id > 0) {
                        let _ = gitlab_tokens::post_gitlab_commit_status(
                            token,
                            gitlab_project_id,
                            &sha_clone,
                            "failed",
                            &format!("Preview deploy failed: {}", e),
                            None,
                        )
                        .await
                        .map_err(|e2| {
                            tracing::warn!(
                                "[webhook/gitlab-mr/{}] failed to post failure status: {}",
                                app_id_clone,
                                e2
                            );
                        });
                    }
                }
                // On success, deploy_preview() itself posts the "success" commit status
                // via the GitLab token path in core.rs.
            });

            tracing::info!(
                "[webhook/gitlab-mr/{}] action={} MR!{} — spawned preview deploy",
                app_id,
                action,
                mr_iid
            );
        }

        "close" | "merge" => {
            let existing: Vec<PreviewDeployment> = sqlx::query_as(
                "SELECT * FROM preview_deployments
                 WHERE app_id = ? AND pr_number = ? AND status != 'expired'",
            )
            .bind(&app.id)
            .bind(mr_iid)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            let mut expired = 0usize;
            for p in existing {
                if let Some(ref cid) = p.container_id {
                    let _ = docker::stop_container(&state.docker, cid).await;
                    let _ = docker::remove_container(&state.docker, cid).await;
                }
                let _ =
                    sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
                        .bind(&p.id)
                        .execute(&state.db)
                        .await;
                expired += 1;
            }
            tracing::info!(
                "[webhook/gitlab-mr/{}] MR!{} {} — expired {} preview(s)",
                app_id,
                mr_iid,
                action,
                expired
            );
        }

        other => {
            tracing::debug!("[webhook/gitlab-mr/{}] ignoring action={}", app_id, other);
        }
    }

    Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response())
}

/// Extract the short branch name from a GitLab push payload.
/// `ref` field is "refs/heads/<branch>".
fn extract_gitlab_branch(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let r = v.get("ref")?.as_str()?;
    Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
}

// ── Generic ────────────────────────────────────────────────────────────────

/// POST /api/webhooks/generic/{app_id}
///
/// Requires `Authorization: Bearer <tond-api-key>` validated against the
/// api_keys table. Any authenticated POST triggers a deploy when auto_deploy
/// is true on the app.
pub async fn generic_webhook(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // ── Require a valid Tond API key ──
    let raw_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if raw_key.is_empty() {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Authorization header required")),
        )
            .into_response());
    }

    let valid = auth::validate_key(&state.db, raw_key)
        .await
        .map_err(|e| AppError(tond_common::TondError::Internal(e.to_string())))?;

    if valid.is_none() {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("invalid API key")),
        )
            .into_response());
    }

    trigger_webhook_deploy(&state, &app_id, "generic", None).await
}

// ── GitHub App Events ──────────────────────────────────────────────────────
//
// POST /webhooks/source/github/events
//
// Receives ALL webhook events from a registered GitHub App (push +
// pull_request). Validates the X-Hub-Signature-256 HMAC using the stored
// `webhook_secret` from the github_apps table (not a per-app secret).
//
// Event routing:
//   push        → find the app(s) whose source_url matches the repo HTTPS URL
//                 and trigger_webhook_deploy for each matching one
//   pull_request → future: route to preview deploy handler (Phase 4.6)
//   everything else → accept silently (200)

/// POST /webhooks/source/github/events
pub async fn github_app_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let delivery = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();

    tracing::debug!("[github-app-events] delivery={} event={}", delivery, event);

    // ── Find the registered GitHub App and validate the HMAC ──
    // GitHub sends X-GitHub-App-ID with the numeric app_id; use it to find the
    // exact row. Falls back to the most-recently-created confirmed row if the
    // header is absent (older GitHub Enterprise or edge cases).
    let github_app_id_header: Option<i64> = headers
        .get("x-github-app-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let gh_app: Option<tond_common::GitHubApp> = if let Some(app_id) = github_app_id_header {
        sqlx::query_as("SELECT * FROM github_apps WHERE app_id = ? LIMIT 1")
            .bind(app_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
    } else {
        sqlx::query_as(
            "SELECT * FROM github_apps WHERE app_id != 0 ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None)
    };

    let Some(gh_app) = gh_app else {
        // No GitHub App registered — silently accept
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    };

    // Validate HMAC signature
    let sig_header = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !verify_github_signature(sig_header, &body, &gh_app.webhook_secret) {
        tracing::warn!("[github-app-events] invalid HMAC signature");
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("invalid signature")),
        )
            .into_response();
    }

    match event.as_str() {
        "push" => handle_github_app_push(&state, &body).await,
        "pull_request" => handle_github_app_pull_request(&state, &gh_app, &body).await,
        _ => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
    }
}

/// Handle a push event from a GitHub App webhook.
/// Finds all apps whose source_url matches the repo's HTTPS clone URL and
/// auto_deploy is enabled, then triggers a deploy for each.
async fn handle_github_app_push(state: &AppState, body: &Bytes) -> Response {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("invalid JSON")),
        )
            .into_response();
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
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    };

    // Find all apps that match this repo URL
    let apps: Vec<tond_common::App> =
        sqlx::query_as("SELECT * FROM apps WHERE source_url = ? AND source_type = 'git'")
            .bind(&repo_url)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    if apps.is_empty() {
        tracing::debug!("[github-app-push] no apps matched repo {}", repo_url);
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    }

    let mut triggered = 0usize;
    for app in apps {
        // Re-use trigger_webhook_deploy to get branch filtering + auto_deploy check
        match trigger_webhook_deploy(state, &app.id, "github-app", branch.as_deref()).await {
            Ok(resp) if resp.status() == StatusCode::ACCEPTED => triggered += 1,
            _ => {}
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

/// Handle a pull_request event from a GitHub App webhook.
///
/// Actions handled:
///   "opened" | "synchronize" | "reopened"
///     – Find all apps whose `source_url` matches the repo's clone URL.
///     – Expire any existing preview deployments for this app+PR.
///     – Insert a new `preview_deployments` row (status = "building") with
///       PR metadata (`pr_number`, `head_sha`, `head_ref`, `pr_url`).
///     – Post a "pending" GitHub commit status.
///     – Spawn `deploy_preview()` in the background.
///
///   "closed"
///     – Find and expire (stop container + mark expired) all preview rows
///       for this app+PR combination.
async fn handle_github_app_pull_request(
    state: &AppState,
    gh_app: &GitHubApp,
    body: &Bytes,
) -> Response {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("invalid JSON")),
        )
            .into_response();
    };

    let action = payload["action"].as_str().unwrap_or("").to_string();
    let pr_number = payload["number"].as_i64().unwrap_or(0);
    let head_sha = payload["pull_request"]["head"]["sha"]
        .as_str()
        .map(|s| s.to_string());
    let head_ref = payload["pull_request"]["head"]["ref"]
        .as_str()
        .map(|s| s.to_string());
    let pr_url = payload["pull_request"]["html_url"]
        .as_str()
        .map(|s| s.to_string());
    let repo_clone_url = payload["repository"]["clone_url"]
        .as_str()
        .or_else(|| payload["repository"]["html_url"].as_str())
        .map(|s| s.to_string());
    let repo_full_name = payload["repository"]["full_name"]
        .as_str()
        .map(|s| s.to_string());

    let Some(repo_url) = repo_clone_url else {
        tracing::warn!("[github-app-pr] no repository.clone_url in payload");
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    };

    // Find all apps linked to this repo
    let apps: Vec<App> =
        sqlx::query_as("SELECT * FROM apps WHERE source_url = ? AND source_type = 'git'")
            .bind(&repo_url)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    if apps.is_empty() {
        tracing::debug!("[github-app-pr] no apps matched repo {}", repo_url);
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    }

    match action.as_str() {
        "opened" | "synchronize" | "reopened" => {
            let Some(ref sha) = head_sha else {
                tracing::warn!("[github-app-pr] missing head.sha in payload");
                return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
            };

            // Derive owner/repo for commit status calls
            let owner_repo = repo_full_name.as_deref().and_then(|full| {
                let mut parts = full.splitn(2, '/');
                let owner = parts.next()?.to_string();
                let repo = parts.next()?.to_string();
                Some((owner, repo))
            });

            // Previews are served only under the preview domain (see domains.rs).
            let preview_domain: Option<String> = {
                let s = state.settings.read().await;
                Some(s.preview_domain.clone()).filter(|d| !d.is_empty())
            };

            let mut deployed = 0usize;

            for app in apps {
                // Skip apps that have not opted in to PR preview deployments
                if app.preview_enabled == 0 {
                    tracing::debug!(
                        "[github-app-pr] skipping app {} — preview_enabled=false",
                        app.name
                    );
                    continue;
                }
                let Some(ref preview_domain) = preview_domain else {
                    tracing::warn!(
                        "[github-app-pr] skipping app {} — no preview domain configured",
                        app.name
                    );
                    continue;
                };

                // Expire any existing previews for this app+PR (best-effort).
                // Save the most recent comment_id so it can be carried over to the
                // new row and the existing PR comment gets updated rather than duplicated.
                let existing: Vec<PreviewDeployment> = sqlx::query_as(
                    "SELECT * FROM preview_deployments
                     WHERE app_id = ? AND pr_number = ? AND status != 'expired'",
                )
                .bind(&app.id)
                .bind(pr_number)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();

                // Pick the comment_id from the most recently created old row.
                let carried_comment_id: Option<i64> =
                    existing.iter().filter_map(|p| p.comment_id).next_back();

                for old in existing {
                    if let Some(ref cid) = old.container_id {
                        let _ = docker::stop_container(&state.docker, cid).await;
                        let _ = docker::remove_container(&state.docker, cid).await;
                    }
                    let _ = sqlx::query(
                        "UPDATE preview_deployments SET status = 'expired' WHERE id = ?",
                    )
                    .bind(&old.id)
                    .execute(&state.db)
                    .await;
                }

                // Build the new preview row
                let slug = {
                    use tond_common::generate_raw_key;
                    generate_raw_key()[..12].to_string()
                };
                let ttl_seconds: i64 = 86400; // 24 h
                let now = chrono::Utc::now();
                let expires_at = (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339();
                let created_at = now.to_rfc3339();
                let subdomain = preview_host(&slug, &app.name, preview_domain);
                let preview_id = uuid::Uuid::new_v4().to_string();

                let insert = sqlx::query(
                    "INSERT INTO preview_deployments
                     (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at,
                      pr_number, head_sha, head_ref, pr_url, comment_id)
                     VALUES (?, ?, ?, ?, ?, 'building', ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&preview_id)
                .bind(&app.id)
                .bind(&slug)
                .bind(&subdomain)
                .bind(ttl_seconds)
                .bind(&expires_at)
                .bind(&created_at)
                .bind(pr_number)
                .bind(sha)
                .bind(head_ref.as_deref())
                .bind(pr_url.as_deref())
                .bind(carried_comment_id)
                .execute(&state.db)
                .await;

                if let Err(e) = insert {
                    tracing::error!("[github-app-pr] failed to insert preview row: {}", e);
                    continue;
                }

                // Post "pending" commit status
                if let Some((ref owner, ref repo)) = owner_repo {
                    let _ = github_apps::post_commit_status(
                        gh_app,
                        owner,
                        repo,
                        sha,
                        "pending",
                        "Building preview environment…",
                        None,
                        "tond/preview",
                    )
                    .await
                    .map_err(|e| {
                        tracing::warn!("[github-app-pr] failed to post pending status: {}", e);
                    });
                }

                // Fetch the just-inserted row and spawn deploy
                let preview_row = sqlx::query_as::<_, PreviewDeployment>(
                    "SELECT * FROM preview_deployments WHERE id = ?",
                )
                .bind(&preview_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();

                let mut preview = match preview_row {
                    Some(p) => p,
                    None => {
                        // Construct a synthetic fallback
                        PreviewDeployment {
                            id: preview_id.clone(),
                            app_id: app.id.clone(),
                            slug: slug.clone(),
                            subdomain: Some(subdomain.clone()),
                            container_id: None,
                            host_port: None,
                            image_tag: None,
                            ttl_seconds,
                            status: "building".to_string(),
                            expires_at: expires_at.clone(),
                            created_at: created_at.clone(),
                            project_preview_id: None,
                            pr_number: Some(pr_number),
                            head_sha: Some(sha.clone()),
                            head_ref: head_ref.clone(),
                            pr_url: pr_url.clone(),
                            comment_id: None,
                            gitlab_project_id: None,
                        }
                    }
                };

                let state_clone = state.clone();
                let app_clone = app.clone();
                let gh_app_clone = gh_app.clone();
                let owner_repo_clone = owner_repo.clone();
                let sha_clone = sha.clone();

                tokio::spawn(async move {
                    if let Err(e) =
                        deploy::deploy_preview(&state_clone, &app_clone, &mut preview).await
                    {
                        tracing::error!(
                            "[github-app-pr] preview deploy failed for {}: {}",
                            app_clone.name,
                            e
                        );
                        let _ = sqlx::query(
                            "UPDATE preview_deployments SET status = 'failed' WHERE id = ?",
                        )
                        .bind(&preview.id)
                        .execute(&state_clone.db)
                        .await;

                        // Post "failure" commit status
                        if let Some((ref owner, ref repo)) = owner_repo_clone {
                            let _ = github_apps::post_commit_status(
                                &gh_app_clone,
                                owner,
                                repo,
                                &sha_clone,
                                "failure",
                                &format!("Preview deploy failed: {}", e),
                                None,
                                "tond/preview",
                            )
                            .await
                            .map_err(|e2| {
                                tracing::warn!(
                                    "[github-app-pr] failed to post failure status: {}",
                                    e2
                                );
                            });
                        }
                    }
                    // On success, deploy_preview() itself posts the "success" status.
                });

                deployed += 1;
            }

            tracing::info!(
                "[github-app-pr] action={} PR#{} — spawned {} preview deploy(s) for repo {}",
                action,
                pr_number,
                deployed,
                repo_url
            );
        }

        "closed" => {
            // Expire all active previews for this PR across all matching apps
            let existing: Vec<PreviewDeployment> = sqlx::query_as(
                "SELECT pd.* FROM preview_deployments pd
                 JOIN apps a ON pd.app_id = a.id
                 WHERE a.source_url = ? AND pd.pr_number = ? AND pd.status != 'expired'",
            )
            .bind(&repo_url)
            .bind(pr_number)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            let mut expired = 0usize;
            for p in existing {
                if let Some(ref cid) = p.container_id {
                    let _ = docker::stop_container(&state.docker, cid).await;
                    let _ = docker::remove_container(&state.docker, cid).await;
                }
                let _ =
                    sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
                        .bind(&p.id)
                        .execute(&state.db)
                        .await;
                expired += 1;
            }

            tracing::info!(
                "[github-app-pr] PR#{} closed — expired {} preview(s) for repo {}",
                pr_number,
                expired,
                repo_url
            );
        }

        other => {
            tracing::debug!("[github-app-pr] ignoring action={}", other);
        }
    }

    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
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
    use tond_common::{ApiKey, ServerSettings};

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

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

    async fn seed_git_app(state: &AppState, name: &str) -> App {
        let mut app = App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.auto_deploy = 1;

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
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
        .unwrap_or_else(|err| panic!("failed to seed webhook app: {err}"));

        app
    }

    async fn insert_api_key(state: &AppState) -> String {
        let (api_key, raw_key) = ApiKey::generate("webhook");
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&api_key.id)
        .bind(&api_key.name)
        .bind(&api_key.key_hash)
        .bind(&api_key.created_at)
        .bind(&api_key.last_used_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed API key: {err}"));
        raw_key
    }

    fn github_signature(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .unwrap_or_else(|err| panic!("failed to initialize HMAC for test signature: {err}"));
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn verify_github_signature_accepts_valid_signature_and_rejects_invalid_formats() {
        let body = br#"{"ref":"refs/heads/main"}"#;
        let secret = "topsecret";
        let signature = github_signature(secret, body);

        assert!(
            verify_github_signature(&signature, body, secret),
            "verify_github_signature should accept a correctly computed HMAC signature"
        );
        assert!(
            !verify_github_signature("sha1=deadbeef", body, secret),
            "verify_github_signature should reject headers with the wrong prefix"
        );
        assert!(
            !verify_github_signature("sha256=not-hex", body, secret),
            "verify_github_signature should reject non-hex digests"
        );
    }

    #[test]
    fn branch_extractors_parse_refs_and_handle_invalid_payloads() {
        assert_eq!(
            extract_github_branch(br#"{"ref":"refs/heads/feature/login"}"#),
            Some("feature/login".to_string()),
            "extract_github_branch should strip refs/heads/ from GitHub push payloads"
        );
        assert_eq!(
            extract_gitlab_branch(br#"{"ref":"refs/heads/main"}"#),
            Some("main".to_string()),
            "extract_gitlab_branch should strip refs/heads/ from GitLab push payloads"
        );
        assert_eq!(
            extract_github_branch(br#"{"unexpected":true}"#),
            None,
            "extract_github_branch should return None when the payload lacks a ref"
        );
        assert_eq!(
            extract_gitlab_branch(b"not json"),
            None,
            "extract_gitlab_branch should return None for malformed JSON payloads"
        );
    }

    #[tokio::test]
    async fn generic_webhook_requires_authorization_header() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;

        let response = generic_webhook(State(state), Path(app.id), HeaderMap::new())
            .await
            .unwrap_or_else(|err| panic!("generic_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "generic_webhook should require a bearer token"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("Authorization header required"),
            "generic_webhook should explain that the Authorization header is required"
        );
    }

    #[tokio::test]
    async fn generic_webhook_rejects_invalid_api_key() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer invalid-key"),
        );

        let response = generic_webhook(State(state), Path(app.id), headers)
            .await
            .unwrap_or_else(|err| panic!("generic_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "generic_webhook should reject invalid bearer tokens"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("invalid API key"),
            "generic_webhook should report invalid API keys"
        );
    }

    #[tokio::test]
    async fn generic_webhook_returns_ok_info_when_auto_deploy_is_disabled() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET auto_deploy = 0 WHERE id = ?")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to disable auto_deploy: {err}"));
        let raw_key = insert_api_key(&state).await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {raw_key}"))
                .unwrap_or_else(|err| panic!("failed to build authorization header: {err}")),
        );

        let response = generic_webhook(State(state), Path(app.id), headers)
            .await
            .unwrap_or_else(|err| panic!("generic_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "generic_webhook should accept authenticated requests even when auto_deploy is disabled"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("auto_deploy not enabled"),
            "generic_webhook should return the documented informational message when auto_deploy is off"
        );
    }

    #[tokio::test]
    async fn github_webhook_rejects_invalid_signature_when_secret_is_configured() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET webhook_secret = ? WHERE id = ?")
            .bind("topsecret")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to set webhook_secret: {err}"));
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", HeaderValue::from_static("push"));
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_static("sha256=deadbeef"),
        );

        let response = github_webhook(
            State(state),
            Path(app.id),
            headers,
            Bytes::from_static(br#"{"ref":"refs/heads/main"}"#),
        )
        .await
        .unwrap_or_else(|err| panic!("github_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "github_webhook should reject requests with invalid signatures when a secret is configured"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("invalid signature"),
            "github_webhook should report invalid signature errors"
        );
    }

    #[tokio::test]
    async fn github_webhook_ignores_non_push_events() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", HeaderValue::from_static("ping"));

        let response = github_webhook(
            State(state),
            Path(app.id),
            headers,
            Bytes::from_static(br#"{}"#),
        )
        .await
        .unwrap_or_else(|err| panic!("github_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "github_webhook should silently accept non-push events"
        );
        let body = response_json(response).await;
        assert!(
            body["success"].as_bool().unwrap_or(false),
            "non-push events should still return a success envelope"
        );
    }

    #[tokio::test]
    async fn github_webhook_returns_branch_not_watched_for_mismatched_branch() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET watch_branch = ? WHERE id = ?")
            .bind("main")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to set watch_branch: {err}"));
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", HeaderValue::from_static("push"));

        let response = github_webhook(
            State(state),
            Path(app.id),
            headers,
            Bytes::from_static(br#"{"ref":"refs/heads/feature-branch"}"#),
        )
        .await
        .unwrap_or_else(|err| panic!("github_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "github_webhook should not fail when the pushed branch is not watched"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("branch not watched"),
            "github_webhook should return an informational response for non-watched branches"
        );
    }

    #[tokio::test]
    async fn gitlab_webhook_rejects_invalid_token_when_secret_is_configured() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET webhook_secret = ? WHERE id = ?")
            .bind("secret-token")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to set webhook_secret: {err}"));
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));
        headers.insert("x-gitlab-token", HeaderValue::from_static("wrong-token"));

        let response = gitlab_webhook(
            State(state),
            Path(app.id),
            headers,
            Bytes::from_static(br#"{"ref":"refs/heads/main"}"#),
        )
        .await
        .unwrap_or_else(|err| panic!("gitlab_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "gitlab_webhook should reject requests with the wrong X-Gitlab-Token when a secret is configured"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("invalid token"),
            "gitlab_webhook should report invalid token errors"
        );
    }

    #[tokio::test]
    async fn gitlab_webhook_returns_branch_not_watched_for_mismatched_branch() {
        let state = test_state().await;
        let app = seed_git_app(&state, "demo").await;
        sqlx::query("UPDATE apps SET watch_branch = ? WHERE id = ?")
            .bind("main")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to set watch_branch: {err}"));
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-event", HeaderValue::from_static("Push Hook"));

        let response = gitlab_webhook(
            State(state),
            Path(app.id),
            headers,
            Bytes::from_static(br#"{"ref":"refs/heads/release"}"#),
        )
        .await
        .unwrap_or_else(|err| panic!("gitlab_webhook should return a response: {}", err.0));

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "gitlab_webhook should not fail when the pushed branch is not watched"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("branch not watched"),
            "gitlab_webhook should return an informational response for non-watched branches"
        );
    }
}
