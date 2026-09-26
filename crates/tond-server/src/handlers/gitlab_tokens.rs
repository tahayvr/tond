// handlers/gitlab_tokens.rs — GitLab Personal Access Token management
//
// Endpoints:
//
//   GET    /api/gitlab-tokens
//     List all stored GitLab tokens. The PAT value is never returned.
//
//   POST   /api/gitlab-tokens
//     Store a new PAT. The token is validated by calling GET <gitlab_url>/api/v4/user
//     before saving. The username returned by GitLab is stored for display.
//     The PAT itself is AES-256-GCM encrypted at rest (same scheme as registry_credentials).
//
//   DELETE /api/gitlab-tokens/{id}
//     Remove a stored token. Also detaches it from any apps that reference it
//     (handled automatically by the ON DELETE SET NULL FK constraint).
//
//   GET    /api/gitlab-tokens/{id}/repos
//     Fetch accessible projects from GitLab using the stored PAT and return
//     a flattened list suitable for a repo picker UI.
//
//   GET    /api/gitlab-tokens/{id}/repos/{project_id}/branches
//     Fetch branches for a specific GitLab project.
//
//   PUT    /api/apps/{id}/gitlab-token
//     Attach or detach a GitLab token from an app.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, AppResponse, CreateGitLabTokenRequest, GitLabBranchResponse, GitLabRepoResponse,
    GitLabToken, GitLabTokenResponse, SetGitLabTokenRequest, TondError,
};
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAdmin, RequireAuth},
    deploy::GitHttpAuth,
    error::AppError,
    secret_key::{self, Purpose},
    state::AppState,
};

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/gitlab-tokens
pub async fn list_gitlab_tokens(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let tokens = sqlx::query_as::<_, GitLabToken>("SELECT * FROM gitlab_tokens ORDER BY name")
        .fetch_all(&state.db)
        .await?;

    let responses: Vec<GitLabTokenResponse> = tokens.into_iter().map(Into::into).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

/// POST /api/gitlab-tokens
pub async fn create_gitlab_token(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(payload): Json<CreateGitLabTokenRequest>,
) -> Result<impl IntoResponse, AppError> {
    if payload.name.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name is required".to_string(),
        )));
    }
    if payload.token.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "token is required".to_string(),
        )));
    }

    let gitlab_url = payload.gitlab_url.trim_end_matches('/').to_string();

    // Validate the token by calling the GitLab /api/v4/user endpoint
    let user_url = format!("{}/api/v4/user", gitlab_url);
    let client = reqwest::Client::new();
    let resp = client
        .get(&user_url)
        .header("PRIVATE-TOKEN", payload.token.trim())
        .send()
        .await
        .map_err(|e: reqwest::Error| {
            AppError(TondError::InvalidInput(format!(
                "Could not reach GitLab at {}: {}",
                gitlab_url, e
            )))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError(TondError::InvalidInput(format!(
            "GitLab token validation failed (HTTP {}): {}",
            status,
            body.chars().take(200).collect::<String>()
        ))));
    }

    let user_json: serde_json::Value =
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e: reqwest::Error| {
                AppError(TondError::Internal(format!(
                    "Failed to parse GitLab user response: {}",
                    e
                )))
            })?;

    let username = user_json
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Encrypt the PAT
    let token_enc = secret_key::encrypt(Purpose::GitlabToken, payload.token.trim())
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO gitlab_tokens (id, name, token_enc, gitlab_url, username)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(payload.name.trim())
    .bind(&token_enc)
    .bind(&gitlab_url)
    .bind(&username)
    .execute(&state.db)
    .await?;

    let token = sqlx::query_as::<_, GitLabToken>("SELECT * FROM gitlab_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(GitLabTokenResponse::from(token))),
    ))
}

/// DELETE /api/gitlab-tokens/{id}
pub async fn delete_gitlab_token(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rows_affected = sqlx::query("DELETE FROM gitlab_tokens WHERE id = ?")
        .bind(&token_id)
        .execute(&state.db)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(AppError(TondError::NotFound(format!(
            "GitLab token {} not found",
            token_id
        ))));
    }

    Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))))
}

/// GET /api/gitlab-tokens/{id}/repos
pub async fn list_gitlab_repos(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let token = fetch_token(&state, &token_id).await?;
    let pat = secret_key::decrypt(Purpose::GitlabToken, &token.token_enc).map_err(|e| {
        AppError(TondError::Internal(format!(
            "Failed to decrypt token: {}",
            e
        )))
    })?;

    let client = reqwest::Client::new();
    let mut all_repos: Vec<GitLabRepoResponse> = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!(
            "{}/api/v4/projects?membership=true&per_page=100&page={}",
            token.gitlab_url, page
        );
        let resp = client
            .get(&url)
            .header("PRIVATE-TOKEN", &pat)
            .send()
            .await
            .map_err(|e: reqwest::Error| {
                AppError(TondError::Internal(format!(
                    "GitLab API request failed: {}",
                    e
                )))
            })?;

        if !resp.status().is_success() {
            return Err(AppError(TondError::Internal(format!(
                "GitLab projects API returned {}",
                resp.status()
            ))));
        }

        let projects: Vec<serde_json::Value> = resp
            .json::<Vec<serde_json::Value>>()
            .await
            .map_err(|e: reqwest::Error| {
                AppError(TondError::Internal(format!(
                    "Failed to parse GitLab projects response: {}",
                    e
                )))
            })?;

        if projects.is_empty() {
            break;
        }

        for p in &projects {
            let repo = GitLabRepoResponse {
                id: p.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                path_with_namespace: p
                    .get("path_with_namespace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                name: p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                visibility: p
                    .get("visibility")
                    .and_then(|v| v.as_str())
                    .unwrap_or("private")
                    .to_string(),
                http_url_to_repo: p
                    .get("http_url_to_repo")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                ssh_url_to_repo: p
                    .get("ssh_url_to_repo")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                default_branch: p
                    .get("default_branch")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            };
            all_repos.push(repo);
        }

        if projects.len() < 100 {
            break;
        }
        page += 1;
    }

    Ok((StatusCode::OK, Json(ApiResponse::ok(all_repos))))
}

/// GET /api/gitlab-tokens/{id}/repos/{project_id}/branches
pub async fn list_gitlab_branches(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path((token_id, project_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let token = fetch_token(&state, &token_id).await?;
    let pat = secret_key::decrypt(Purpose::GitlabToken, &token.token_enc).map_err(|e| {
        AppError(TondError::Internal(format!(
            "Failed to decrypt token: {}",
            e
        )))
    })?;

    let client = reqwest::Client::new();
    let url = format!(
        "{}/api/v4/projects/{}/repository/branches?per_page=100",
        token.gitlab_url,
        urlencoding::encode(&project_id)
    );

    let resp = client
        .get(&url)
        .header("PRIVATE-TOKEN", &pat)
        .send()
        .await
        .map_err(|e: reqwest::Error| {
            AppError(TondError::Internal(format!(
                "GitLab API request failed: {}",
                e
            )))
        })?;

    if !resp.status().is_success() {
        return Err(AppError(TondError::Internal(format!(
            "GitLab branches API returned {}",
            resp.status()
        ))));
    }

    let branches_raw: Vec<serde_json::Value> = resp
        .json::<Vec<serde_json::Value>>()
        .await
        .map_err(|e: reqwest::Error| {
            AppError(TondError::Internal(format!(
                "Failed to parse GitLab branches response: {}",
                e
            )))
        })?;

    let branches: Vec<GitLabBranchResponse> = branches_raw
        .iter()
        .filter_map(|b| {
            let name = b.get("name")?.as_str()?.to_string();
            Some(GitLabBranchResponse { name })
        })
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(branches))))
}

/// PUT /api/apps/{id}/gitlab-token
///
/// Attach or detach a GitLab token from an app.
pub async fn set_app_gitlab_token(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(payload): Json<SetGitLabTokenRequest>,
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

    // If attaching, verify the token exists
    if let Some(ref tid) = payload.gitlab_token_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM gitlab_tokens WHERE id = ?)")
                .bind(tid)
                .fetch_one(&state.db)
                .await?;

        if !exists {
            return Err(AppError(TondError::NotFound(format!(
                "GitLab token {} not found",
                tid
            ))));
        }
    }

    sqlx::query("UPDATE apps SET gitlab_token_id = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&payload.gitlab_token_id)
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

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn fetch_token(state: &AppState, token_id: &str) -> Result<GitLabToken, AppError> {
    sqlx::query_as::<_, GitLabToken>("SELECT * FROM gitlab_tokens WHERE id = ?")
        .bind(token_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            AppError(TondError::NotFound(format!(
                "GitLab token {} not found",
                token_id
            )))
        })
}

/// HTTPS credentials for cloning a GitLab repo with the stored PAT
/// (`oauth2:<pat>` Basic auth). They are passed to git as a header through its
/// environment config, never embedded in the clone URL — see deploy/git.rs.
pub fn gitlab_git_credentials(token: &GitLabToken) -> anyhow::Result<GitHttpAuth> {
    let pat = secret_key::decrypt(Purpose::GitlabToken, &token.token_enc)?;
    Ok(GitHttpAuth {
        username: "oauth2".to_string(),
        password: pat,
    })
}

/// Post a commit status to GitLab (best-effort — errors are logged, not propagated).
///
/// `state` maps to GitLab commit status states: "pending", "running", "success", "failed",
/// "canceled".
/// `gitlab_project_id` is the numeric project ID (from `project.id` in webhook payloads).
pub async fn post_gitlab_commit_status(
    token: &GitLabToken,
    gitlab_project_id: i64,
    sha: &str,
    state_str: &str,
    description: &str,
    target_url: Option<&str>,
) -> anyhow::Result<()> {
    let pat = secret_key::decrypt(Purpose::GitlabToken, &token.token_enc)?;
    let base = token.gitlab_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v4/projects/{}/statuses/{}",
        base, gitlab_project_id, sha
    );
    let mut body = serde_json::json!({
        "state": state_str,
        "description": description,
        "context": "tond/preview",
    });
    if let Some(u) = target_url {
        body["target_url"] = serde_json::Value::String(u.to_string());
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("PRIVATE-TOKEN", &pat)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitLab status API {} {}: {}", url, status, text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::to_bytes,
        extract::{Path, State},
        response::IntoResponse,
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::GitLabToken;
    use tond_common::{ServerSettings, SetGitLabTokenRequest};

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for GitLab tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    fn sample_token() -> GitLabToken {
        GitLabToken {
            id: "token-1".to_string(),
            name: "GitLab PAT".to_string(),
            token_enc: secret_key::encrypt(Purpose::GitlabToken, "secret-pat")
                .unwrap_or_else(|err| panic!("PAT encryption should succeed: {err}")),
            gitlab_url: "https://gitlab.example.com/".to_string(),
            username: "alice".to_string(),
            created_at: "2026-04-08T12:00:00Z".to_string(),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_source_app(state: &AppState, id: &str, name: &str) {
        let app = tond_common::App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://gitlab.example.com/group/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_shared_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
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
        .bind(&app.preview_shared_keys)
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

    #[tokio::test]
    async fn fetch_token_returns_seeded_row_and_not_found_for_missing_id() {
        let state = test_state().await;
        let token = sample_token();

        sqlx::query(
            "INSERT INTO gitlab_tokens (id, name, token_enc, gitlab_url, username)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&token.id)
        .bind(&token.name)
        .bind(&token.token_enc)
        .bind(&token.gitlab_url)
        .bind(&token.username)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed GitLab token: {err}"));

        let fetched = fetch_token(&state, "token-1")
            .await
            .unwrap_or_else(|_| panic!("fetch_token should return the seeded token"));
        assert_eq!(
            fetched.name, "GitLab PAT",
            "fetch_token should return the stored GitLab token row"
        );

        let err = fetch_token(&state, "missing")
            .await
            .err()
            .unwrap_or_else(|| panic!("fetch_token should report missing rows"));
        assert!(
            err.0.to_string().contains("GitLab token missing not found"),
            "fetch_token should produce a helpful not-found error for unknown token ids"
        );
    }

    #[test]
    fn gitlab_git_credentials_decrypt_the_pat_for_header_auth() {
        let token = sample_token();
        let auth = gitlab_git_credentials(&token)
            .unwrap_or_else(|err| panic!("PAT decryption should succeed: {err}"));
        assert_eq!(auth.username, "oauth2");
        assert_eq!(auth.password, "secret-pat");
    }

    #[tokio::test]
    async fn set_app_gitlab_token_attaches_and_detaches_existing_tokens() {
        let state = test_state().await;
        let token = sample_token();
        seed_source_app(&state, "app-1", "frontend").await;

        sqlx::query(
            "INSERT INTO gitlab_tokens (id, name, token_enc, gitlab_url, username)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&token.id)
        .bind(&token.name)
        .bind(&token.token_enc)
        .bind(&token.gitlab_url)
        .bind(&token.username)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed GitLab token row: {err}"));

        let response = set_app_gitlab_token(
            crate::auth::rate_limit::RateLimit,
            crate::auth::RequireAdmin,
            State(state.clone()),
            Path("app-1".to_string()),
            Json(SetGitLabTokenRequest {
                gitlab_token_id: Some("token-1".to_string()),
            }),
        )
        .await
        .unwrap_or_else(|_| {
            panic!("set_app_gitlab_token should succeed for existing app and token rows")
        })
        .into_response();
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["gitlab_token_id"],
            serde_json::Value::String("token-1".to_string()),
            "set_app_gitlab_token should persist the attached GitLab token id on the app row"
        );

        let response = set_app_gitlab_token(
            crate::auth::rate_limit::RateLimit,
            crate::auth::RequireAdmin,
            State(state.clone()),
            Path("app-1".to_string()),
            Json(SetGitLabTokenRequest {
                gitlab_token_id: None,
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("set_app_gitlab_token should allow detaching a token"))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["gitlab_token_id"],
            serde_json::Value::Null,
            "set_app_gitlab_token should clear the linked GitLab token when null is provided"
        );
    }
}
