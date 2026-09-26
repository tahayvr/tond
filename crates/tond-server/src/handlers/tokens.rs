// handlers/tokens.rs — API tokens (see auth/tokens.rs for the model)
//
// Routes:
//   GET    /api/v1/tokens/catalog  — scope groups, presets, roles and what the caller may grant
//   GET    /api/v1/tokens          — the caller's tokens; `?all=true` lists everyone's (`tokens:admin`)
//   POST   /api/v1/tokens          — create; the secret is in the response, once
//   DELETE /api/v1/tokens/{id}     — revoke (own tokens, or any with `tokens:admin`)
//   GET    /api/v1/auth/whoami     — who the request authenticated as, with its scopes
//
// Any signed-in user may create tokens up to their own role; the root CLI
// socket creates owner-less tokens (`tond token create`). API tokens can't
// create tokens, and can only list or revoke them with `tokens:admin`.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tond_common::*;

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Principal, RateLimit, principal::Actor, tokens},
    error::AppError,
    state::AppState,
};

// ── GET /api/v1/tokens/catalog ──

pub async fn token_catalog(_rate: RateLimit, auth: Principal) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(ScopeCatalog::new(auth.scopes()))),
    )
}

// ── GET /api/v1/tokens ──

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ListTokensQuery {
    #[serde(default)]
    pub all: bool,
}

pub async fn list_tokens(
    _rate: RateLimit,
    auth: Principal,
    State(state): State<AppState>,
    Query(query): Query<ListTokensQuery>,
) -> Result<impl IntoResponse, AppError> {
    let owner = if query.all || auth.is_local_socket() {
        auth.require(Scope::TokensAdmin)?;
        None
    } else {
        if auth.is_token() {
            auth.require(Scope::TokensAdmin)?;
        }
        auth.user_id()
    };
    let rows = tokens::list_tokens(&state.db, owner).await?;
    let body: Vec<TokenResponse> = rows.iter().map(tokens::TokenRow::to_response).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

// ── POST /api/v1/tokens ──

pub async fn create_token(
    _rate: RateLimit,
    auth: Principal,
    State(state): State<AppState>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse, AppError> {
    let created = tokens::create_token(&state.db, &auth, &req).await?;
    let t = &created.token;
    let scopes: Vec<String> = t.scopes.iter().map(|s| s.to_string()).collect();
    let mut detail = format!("kind={} scopes={}", t.kind, scopes.join(","));
    if !t.app_ids.is_empty() {
        detail.push_str(&format!(" apps={}", t.app_ids.join(",")));
    }
    if !t.project_ids.is_empty() {
        detail.push_str(&format!(" projects={}", t.project_ids.join(",")));
    }
    if !t.allowed_ips.is_empty() {
        detail.push_str(&format!(" ips={}", t.allowed_ips.join(",")));
    }
    detail.push_str(&format!(
        " expires={}",
        t.expires_at.as_deref().unwrap_or("never")
    ));
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("token_create")
            .resource("token", &t.id, &t.name)
            .detail(detail),
    )
    .await;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(created))))
}

// ── DELETE /api/v1/tokens/{id} ──

pub async fn revoke_token(
    _rate: RateLimit,
    auth: Principal,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = tokens::find_token(&state.db, &id)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("token {id}"))))?;
    let own = !auth.is_token()
        && auth.user_id().is_some()
        && auth.user_id() == row.owner_user_id.as_deref();
    if !own {
        auth.require(Scope::TokensAdmin)?;
    }
    let revoked = tokens::revoke_token(&state.db, &row.id).await?;
    if revoked {
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("token_revoke").resource("token", &row.id, &row.name),
        )
        .await;
    }
    let row = tokens::find_token(&state.db, &id).await?.unwrap_or(row);
    Ok((StatusCode::OK, Json(ApiResponse::ok(row.to_response()))))
}

// ── GET /api/v1/auth/whoami ──

#[derive(Debug, Serialize, Deserialize)]
pub struct WhoAmIResponse {
    /// "user" | "token" | "local_socket"
    pub kind: String,
    pub id: Option<String>,
    pub label: String,
    pub role: Option<Role>,
    pub scopes: Vec<Scope>,
    pub app_ids: Vec<String>,
    pub project_ids: Vec<String>,
}

pub async fn whoami(_rate: RateLimit, auth: Principal) -> impl IntoResponse {
    let actor = auth.audit_actor();
    let role = match auth.actor() {
        Actor::User { role, .. } => Some(*role),
        _ => None,
    };
    let (app_ids, project_ids) = auth
        .filter()
        .map(|f| (f.app_ids.clone(), f.project_ids.clone()))
        .unwrap_or_default();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(WhoAmIResponse {
            kind: actor.kind.to_string(),
            id: actor.id,
            label: actor.label,
            role,
            scopes: auth.scopes().to_vec(),
            app_ids,
            project_ids,
        })),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::response::Response;
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;

    use super::*;
    use crate::auth::RateLimiter;

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("docker: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_user(state: &AppState, id: &str, role: &str) -> User {
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, 'h', ?, 'x', 'x')",
        )
        .bind(id)
        .bind(format!("{id}@example.com"))
        .bind(role)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    async fn json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{e}"))
    }

    fn request(name: &str, scopes: &[&str]) -> CreateTokenRequest {
        CreateTokenRequest {
            name: name.into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            ..CreateTokenRequest::default()
        }
    }

    #[tokio::test]
    async fn tokens_are_created_once_listed_without_secrets_and_audited() {
        let state = test_state().await;
        let dev = seed_user(&state, "dev", "deployer").await;
        let auth = Principal::for_user(&dev, None, None);
        let response = create_token(
            RateLimit,
            auth.clone(),
            State(state.clone()),
            Json(CreateTokenRequest {
                preset: Some("deployer".into()),
                kind: Some(TokenKind::Ci),
                ..request("ci", &[])
            }),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json(response).await;
        let secret = body["data"]["secret"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(secret.starts_with("tond_pat_"));
        assert_eq!(body["data"]["token"]["owner_label"], "dev@example.com");
        assert_eq!(
            body["data"]["token"]["scopes"],
            serde_json::json!(["apps:read", "apps:deploy", "logs:read"])
        );
        assert!(
            body["data"]["token"]["expires_at"].is_string(),
            "90-day default"
        );

        let list = list_tokens(
            RateLimit,
            auth,
            State(state.clone()),
            Query(ListTokensQuery { all: false }),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        let text = serde_json::to_string(&json(list).await).unwrap_or_default();
        assert!(text.contains("tond_pat_"), "the visible prefix is listed");
        assert!(!text.contains(&secret), "the secret is never listed");

        let (actor, action): (String, String) =
            sqlx::query_as("SELECT actor_label, action FROM audit_log")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (actor.as_str(), action.as_str()),
            ("dev@example.com", "token_create")
        );
    }

    #[tokio::test]
    async fn a_token_can_never_exceed_its_creators_role() {
        let state = test_state().await;
        let viewer = seed_user(&state, "v", "viewer").await;
        let result = create_token(
            RateLimit,
            Principal::for_user(&viewer, None, None),
            State(state.clone()),
            Json(request("sneaky", &["apps:read", "apps:deploy"])),
        )
        .await;
        let err = result
            .err()
            .unwrap_or_else(|| panic!("viewers can't mint deploy tokens"));
        assert_eq!(err.0.code(), "SCOPE_DENIED");
        assert!(err.0.to_string().contains("apps:deploy"));
    }

    #[tokio::test]
    async fn agent_tokens_refuse_admin_scopes_and_must_expire() {
        let state = test_state().await;
        let admin = seed_user(&state, "a", "admin").await;
        let auth = Principal::for_user(&admin, None, None);
        let admin_scope = create_token(
            RateLimit,
            auth.clone(),
            State(state.clone()),
            Json(CreateTokenRequest {
                kind: Some(TokenKind::Agent),
                ..request("bot", &["apps:read", "users:admin"])
            }),
        )
        .await;
        assert!(admin_scope.is_err());
        let forever = create_token(
            RateLimit,
            auth.clone(),
            State(state.clone()),
            Json(CreateTokenRequest {
                kind: Some(TokenKind::Agent),
                expires_in_hours: Some(0),
                ..request("bot", &["apps:read"])
            }),
        )
        .await;
        assert!(forever.is_err());
        let ok = create_token(
            RateLimit,
            auth,
            State(state.clone()),
            Json(CreateTokenRequest {
                kind: Some(TokenKind::Agent),
                preset: Some("agent-operator".into()),
                ..request("bot", &[])
            }),
        )
        .await;
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn tokens_cant_mint_tokens_and_only_owners_or_token_admins_revoke() {
        let state = test_state().await;
        let dev = seed_user(&state, "dev", "deployer").await;
        let other = seed_user(&state, "other", "deployer").await;
        let created = tokens::create_token(
            &state.db,
            &Principal::for_user(&dev, None, None),
            &request("mine", &["apps:read"]),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));

        let as_token = Principal::test_token(ScopeSet::all(), None);
        assert!(
            tokens::create_token(&state.db, &as_token, &request("child", &["apps:read"]))
                .await
                .is_err()
        );

        let denied = revoke_token(
            RateLimit,
            Principal::for_user(&other, None, None),
            State(state.clone()),
            Path(created.token.id.clone()),
        )
        .await;
        assert!(denied.is_err(), "someone else's token needs tokens:admin");

        let response = revoke_token(
            RateLimit,
            Principal::for_user(&dev, None, None),
            State(state.clone()),
            Path(created.token.id.clone()),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        assert!(json(response).await["data"]["revoked_at"].is_string());
    }

    #[tokio::test]
    async fn resource_filters_resolve_app_names_to_ids() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO apps (id, name, created_at, updated_at) VALUES ('app-1', 'web', 'x', 'x')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let created = tokens::create_token(
            &state.db,
            &Principal::local_socket(),
            &CreateTokenRequest {
                app_ids: vec!["web".into()],
                allowed_ips: vec!["10.0.0.0/8".into()],
                ..request("ci", &["apps:deploy"])
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(created.token.app_ids, vec!["app-1"]);
        assert_eq!(created.token.owner_label, "root (host)");
        assert!(
            tokens::create_token(
                &state.db,
                &Principal::local_socket(),
                &CreateTokenRequest {
                    app_ids: vec!["missing".into()],
                    ..request("ci", &["apps:deploy"])
                },
            )
            .await
            .is_err()
        );
        assert!(
            tokens::create_token(
                &state.db,
                &Principal::local_socket(),
                &CreateTokenRequest {
                    allowed_ips: vec!["nope".into()],
                    ..request("ci", &["apps:deploy"])
                },
            )
            .await
            .is_err()
        );
    }
}
