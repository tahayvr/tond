// handlers/gitea_tokens.rs — Gitea/Forgejo access tokens
//
//   GET    /api/gitea-tokens           — stored tokens (never the token itself)
//   POST   /api/gitea-tokens           — store a token (checked against
//                                        <base_url>/api/v1/user first)
//   DELETE /api/gitea-tokens/{id}      — remove (apps using it are detached)
//   PUT    /api/apps/{id}/gitea-token  — link or unlink a token and an app
//
// A linked token lets Tond clone the app's private repository, set commit
// statuses (tond/deploy, tond/preview) and comment the preview URL on pull
// requests (forge.rs). It needs the repository read/write scope ("write:
// repository" on Gitea 1.20+ / Forgejo) and "write:issue" for comments.
// Encrypted at rest (Purpose::GiteaToken).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, AppResponse, CreateGiteaTokenRequest, GiteaToken, GiteaTokenResponse,
    SetGiteaTokenRequest, TondError,
};
use uuid::Uuid;

use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    deploy::GitHttpAuth,
    error::AppError,
    secret_key::{self, Purpose},
    state::AppState,
};

/// Validate and normalise an instance URL: http(s), no path beyond the
/// optional sub-path, no credentials, no trailing slash.
pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| "The instance URL must start with https:// (or http://)".to_string())?;
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() || host.contains('@') || rest.contains(['?', '#', ' ']) {
        return Err(format!(
            "'{raw}' isn't a valid instance URL, e.g. https://codeberg.org"
        ));
    }
    Ok(url.to_string())
}

/// HTTPS clone credentials for a Gitea token (Basic auth, token as the
/// password), passed to git through its environment, never in the URL.
pub fn gitea_git_credentials(token: &GiteaToken) -> anyhow::Result<GitHttpAuth> {
    let pat = secret_key::decrypt(Purpose::GiteaToken, &token.token_enc)?;
    Ok(GitHttpAuth {
        username: if token.username.is_empty() {
            "tond".to_string()
        } else {
            token.username.clone()
        },
        password: pat,
    })
}

/// GET /api/gitea-tokens
pub async fn list_gitea_tokens(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let tokens = sqlx::query_as::<_, GiteaToken>("SELECT * FROM gitea_tokens ORDER BY name")
        .fetch_all(&state.db)
        .await?;
    let out: Vec<GiteaTokenResponse> = tokens.into_iter().map(Into::into).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(out))))
}

/// POST /api/gitea-tokens
pub async fn create_gitea_token(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateGiteaTokenRequest>,
) -> Result<impl IntoResponse, AppError> {
    let name = req.name.trim();
    let token = req.token.trim();
    if name.is_empty() || token.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "A name and a token are required".to_string(),
        )));
    }
    let base_url =
        normalize_base_url(&req.base_url).map_err(|m| AppError(TondError::InvalidInput(m)))?;

    // Check the token before storing it, and learn whose it is.
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/api/v1/user"))
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", "tond-server")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "Couldn't reach {base_url}: {e}"
            )))
        })?;
    if !resp.status().is_success() {
        return Err(AppError(TondError::InvalidInput(format!(
            "{base_url} refused the token (HTTP {}). Create one under Settings → Applications \
             with repository and issue write access.",
            resp.status()
        ))));
    }
    let user: serde_json::Value = resp.json().await.unwrap_or_default();
    let username = user["login"]
        .as_str()
        .or_else(|| user["username"].as_str())
        .unwrap_or_default()
        .to_string();

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO gitea_tokens (id, name, token_enc, base_url, username) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(secret_key::encrypt(Purpose::GiteaToken, token)?)
    .bind(&base_url)
    .bind(&username)
    .execute(&state.db)
    .await?;
    let stored = sqlx::query_as::<_, GiteaToken>("SELECT * FROM gitea_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("gitea_token_create").resource("gitea_token", &stored.id, &stored.name),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(GiteaTokenResponse::from(stored))),
    ))
}

/// DELETE /api/gitea-tokens/{id}
pub async fn delete_gitea_token(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM gitea_tokens WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?;
    let name = name.ok_or_else(|| AppError(TondError::NotFound(format!("Gitea token {id}"))))?;
    sqlx::query("DELETE FROM gitea_tokens WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("gitea_token_delete").resource("gitea_token", &id, &name),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))))
}

/// PUT /api/apps/{id}/gitea-token
pub async fn set_app_gitea_token(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(req): Json<SetGiteaTokenRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&app_id)
    .bind(&app_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::AppNotFound(app_id.clone())))?;

    if let Some(tid) = &req.gitea_token_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM gitea_tokens WHERE id = ?)")
                .bind(tid)
                .fetch_one(&state.db)
                .await?;
        if !exists {
            return Err(AppError(TondError::NotFound(format!("Gitea token {tid}"))));
        }
    }
    sqlx::query("UPDATE apps SET gitea_token_id = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&req.gitea_token_id)
        .bind(&app.id)
        .execute(&state.db)
        .await?;
    let updated = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps WHERE id = ?")
        .bind(&app.id)
        .fetch_one(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("gitea_token_assign").resource("app", &updated.id, &updated.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_urls_are_normalised_and_checked() {
        assert_eq!(
            normalize_base_url(" https://codeberg.org/ "),
            Ok("https://codeberg.org".to_string())
        );
        assert_eq!(
            normalize_base_url("http://git.lan:3000/gitea"),
            Ok("http://git.lan:3000/gitea".to_string())
        );
        for bad in [
            "codeberg.org",
            "ftp://x",
            "https://",
            "https://u:p@host",
            "https://h/?x",
        ] {
            assert!(normalize_base_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn clone_credentials_use_the_token_owner_and_the_token() {
        let token = GiteaToken {
            id: "g1".to_string(),
            name: "codeberg".to_string(),
            token_enc: secret_key::encrypt(Purpose::GiteaToken, "s3cret")
                .unwrap_or_else(|e| panic!("{e}")),
            base_url: "https://codeberg.org".to_string(),
            username: "me".to_string(),
            created_at: String::new(),
        };
        let auth = gitea_git_credentials(&token).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (auth.username.as_str(), auth.password.as_str()),
            ("me", "s3cret")
        );
    }
}
