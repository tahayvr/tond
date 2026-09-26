// handlers/users.rs — user management (admins: `users:admin`)
//
// Routes:
//   GET    /api/users                          — every user, with live session counts
//   POST   /api/users                          — invite: email + role → one-time set-password link (24 h)
//   PATCH  /api/users/{id}                     — change role and/or disable / enable
//   DELETE /api/users/{id}                     — delete (never yourself, never the last admin)
//   POST   /api/users/{id}/password-link       — new set-password link (reset)
//   GET    /api/users/{id}/sessions            — a user's live sessions
//   DELETE /api/users/{id}/sessions            — sign a user out everywhere
//   DELETE /api/users/{id}/sessions/{session}  — revoke one session
//
// Changing a role or disabling an account signs the user out everywhere.
// Their API tokens don't need rewriting: a token's scopes are capped by its
// owner's *current* role on every request, so a demotion narrows them at once,
// and a disabled owner's tokens are refused until the account is enabled
// again. Deleting a user deletes their tokens.
//
// Admins never pick passwords for other people: invites and resets hand out a
// single-use link and the user chooses a password under the normal policy.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::Serialize;
use tond_common::*;
use uuid::Uuid;

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Authz, RateLimit, scope},
    error::AppError,
    handlers::auth::{
        password_links::{self, LinkPurpose},
        session_responses,
    },
    state::AppState,
};

/// A user as the admin Users page shows it.
#[derive(Debug, Serialize)]
pub struct AdminUserResponse {
    #[serde(flatten)]
    pub user: UserResponse,
    /// Live (not expired, not idle) sessions.
    pub active_sessions: i64,
    /// Whether this is the admin looking at the list.
    pub is_you: bool,
}

async fn fetch_user(db: &sqlx::SqlitePool, id: &str) -> Result<User, AppError> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("user {id}"))))
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError(TondError::InvalidInput(msg.into()))
}

fn conflict(msg: impl Into<String>) -> AppError {
    AppError(TondError::AppAlreadyExists(msg.into()))
}

/// Active admins other than `except`.
async fn other_active_admins(db: &sqlx::SqlitePool, except: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL AND id != ?",
    )
    .bind(except)
    .fetch_one(db)
    .await
}

async fn delete_sessions(db: &sqlx::SqlitePool, user_id: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?
        .rows_affected())
}

fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !email.chars().any(char::is_whitespace)
        && email.len() <= 254
}

// ── GET /api/users ──

pub async fn list_users(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at ASC")
        .fetch_all(&state.db)
        .await?;
    let now = Utc::now();
    let idle_hours = state.settings.read().await.session_idle_timeout_hours;
    let idle_cutoff = (now - chrono::Duration::hours(idle_hours.max(1))).to_rfc3339();
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT user_id, COUNT(*) FROM sessions
         WHERE expires_at > ? AND COALESCE(last_used_at, created_at) > ?
         GROUP BY user_id",
    )
    .bind(now.to_rfc3339())
    .bind(&idle_cutoff)
    .fetch_all(&state.db)
    .await?;
    let me = auth.session_user_id().map(str::to_string);
    let body: Vec<AdminUserResponse> = users
        .into_iter()
        .map(|u| AdminUserResponse {
            active_sessions: counts
                .iter()
                .find(|(id, _)| *id == u.id)
                .map(|(_, n)| *n)
                .unwrap_or(0),
            is_you: me.as_deref() == Some(u.id.as_str()),
            user: UserResponse::from(u),
        })
        .collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

// ── POST /api/users ──

pub async fn create_user(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let email = req.email.trim().to_string();
    if !is_valid_email(&email) {
        return Err(invalid(
            "Enter a valid email address, like name@example.com.",
        ));
    }
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE lower(email) = ?")
        .bind(email.to_lowercase())
        .fetch_optional(&state.db)
        .await?;
    if exists.is_some() {
        return Err(conflict(format!("a user with the email {email}")));
    }

    let now = Utc::now().to_rfc3339();
    let user_id = Uuid::new_v4().to_string();
    // An empty password hash can never verify: the account is unusable until
    // the invited user sets a password through the link.
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, created_at, updated_at)
         VALUES (?, ?, '', ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(&email)
    .bind(req.role.as_str())
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    let (token, expires_at) =
        password_links::create(&state.db, &user_id, LinkPurpose::Invite).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("user_invite")
            .resource("user", &user_id, &email)
            .detail(format!("role={}", req.role)),
    )
    .await;

    let user = fetch_user(&state.db, &user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(PasswordLinkResponse {
            user: UserResponse::from(user),
            path: password_links::link_path(&token),
            token,
            expires_at,
        })),
    ))
}

// ── PATCH /api/users/{id} ──

pub async fn update_user(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    let is_self = auth.session_user_id() == Some(user.id.as_str());
    let current_role = user.role();
    let role_change = req.role.filter(|r| *r != current_role);
    let disable_change = req.disabled.filter(|d| *d != user.disabled_at.is_some());

    if role_change.is_none() && disable_change.is_none() {
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(UserResponse::from(user))),
        ));
    }
    if is_self {
        return Err(invalid(
            "You can't change your own role or disable yourself. Ask another admin.",
        ));
    }
    let loses_admin = current_role == Role::Admin
        && user.disabled_at.is_none()
        && (role_change.is_some() || disable_change == Some(true));
    if loses_admin && other_active_admins(&state.db, &user.id).await? == 0 {
        return Err(invalid(
            "This is the only active admin. Make someone else an admin first.",
        ));
    }

    let now = Utc::now().to_rfc3339();
    let mut changes = Vec::new();
    if let Some(role) = role_change {
        sqlx::query("UPDATE users SET role = ?, updated_at = ? WHERE id = ?")
            .bind(role.as_str())
            .bind(&now)
            .bind(&user.id)
            .execute(&state.db)
            .await?;
        changes.push(format!("role {current_role} → {role}"));
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("user_role_change")
                .resource("user", &user.id, &user.email)
                .detail(format!("{current_role} → {role}")),
        )
        .await;
    }
    if let Some(disabled) = disable_change {
        sqlx::query("UPDATE users SET disabled_at = ?, updated_at = ? WHERE id = ?")
            .bind(disabled.then_some(now.clone()))
            .bind(&now)
            .bind(&user.id)
            .execute(&state.db)
            .await?;
        changes.push(if disabled { "disabled" } else { "enabled" }.to_string());
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new(if disabled {
                "user_disable"
            } else {
                "user_enable"
            })
            .resource("user", &user.id, &user.email),
        )
        .await;
    }
    // New permissions take effect on the next sign-in, everywhere.
    let signed_out = delete_sessions(&state.db, &user.id).await?;
    if signed_out > 0 {
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("session_revoke")
                .resource("user", &user.id, &user.email)
                .detail(format!(
                    "{signed_out} session(s) after {}",
                    changes.join(", ")
                )),
        )
        .await;
    }

    let user = fetch_user(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(UserResponse::from(user))),
    ))
}

// ── DELETE /api/users/{id} ──

pub async fn delete_user(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    if auth.session_user_id() == Some(user.id.as_str()) {
        return Err(invalid(
            "You can't delete your own account. Ask another admin.",
        ));
    }
    if user.role() == Role::Admin && other_active_admins(&state.db, &user.id).await? == 0 {
        return Err(invalid(
            "This is the only active admin. Make someone else an admin first.",
        ));
    }
    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens WHERE owner_user_id = ?")
        .bind(&user.id)
        .fetch_one(&state.db)
        .await?;
    // Sessions, tokens, links, backup codes and terminal tickets cascade.
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&user.id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("user_delete")
            .resource("user", &user.id, &user.email)
            .detail(format!(
                "role={}, {tokens} API token(s) deleted",
                user.role()
            )),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": user.id }))),
    ))
}

// ── POST /api/users/{id}/password-link ──

pub async fn create_password_link(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    let purpose = if user.has_password() {
        LinkPurpose::Reset
    } else {
        LinkPurpose::Invite
    };
    let (token, expires_at) = password_links::create(&state.db, &user.id, purpose).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new(match purpose {
            LinkPurpose::Invite => "user_invite_resend",
            LinkPurpose::Reset => "password_reset_link",
        })
        .resource("user", &user.id, &user.email),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(PasswordLinkResponse {
            user: UserResponse::from(user),
            path: password_links::link_path(&token),
            token,
            expires_at,
        })),
    ))
}

// ── GET /api/users/{id}/sessions ──

pub async fn list_user_sessions(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    let idle_hours = state.settings.read().await.session_idle_timeout_hours;
    let sessions = session_responses(&state.db, &user.id, auth.session_id(), idle_hours).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(sessions))))
}

// ── DELETE /api/users/{id}/sessions ──

pub async fn revoke_user_sessions(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    // Keep the admin's own current session if they sign themselves out elsewhere.
    let n = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id != ?")
        .bind(&user.id)
        .bind(auth.session_id().unwrap_or_default())
        .execute(&state.db)
        .await?
        .rows_affected();
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("session_revoke")
            .resource("user", &user.id, &user.email)
            .detail(format!("{n} session(s)")),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "revoked": n }))),
    ))
}

// ── DELETE /api/users/{id}/sessions/{session_id} ──

pub async fn revoke_user_session(
    _rate: RateLimit,
    auth: Authz<scope::UsersAdmin>,
    State(state): State<AppState>,
    Path((id, session_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let user = fetch_user(&state.db, &id).await?;
    let n = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(&session_id)
        .bind(&user.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError(TondError::NotFound("session".to_string())));
    }
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("session_revoke")
            .resource("user", &user.id, &user.email)
            .detail("1 session"),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "revoked": 1 }))),
    ))
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
    use crate::auth::{Principal, RateLimiter};

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

    async fn seed(state: &AppState, id: &str, role: &str) {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, 'h', ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("{id}@example.com"))
        .bind(role)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("seed user: {e}"));
        sqlx::query("INSERT INTO sessions (id, user_id, expires_at, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)")
            .bind(format!("sess-{id}"))
            .bind(id)
            .bind((Utc::now() + chrono::Duration::days(1)).to_rfc3339())
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("seed session: {e}"));
    }

    /// Admin principal acting as the seeded user "admin".
    async fn admin(state: &AppState) -> Authz<scope::UsersAdmin> {
        let user = fetch_user(&state.db, "admin")
            .await
            .unwrap_or_else(|e| panic!("{:?}", e.0));
        Authz::unchecked(Principal::for_user(&user, None, Some("sess-admin".into())))
    }

    async fn json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{e}"))
    }

    async fn audit_actions(state: &AppState) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY seq")
            .fetch_all(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn invites_create_a_passwordless_user_and_a_link() {
        let state = test_state().await;
        seed(&state, "admin", "admin").await;
        let response = create_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Json(CreateUserRequest {
                email: "new@example.com".into(),
                role: Role::Deployer,
            }),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json(response).await;
        assert_eq!(body["data"]["user"]["role"], "deployer");
        assert_eq!(body["data"]["user"]["password_set"], false);
        let path = body["data"]["path"].as_str().unwrap_or_default();
        assert!(path.starts_with("/set-password?token="));
        assert_eq!(audit_actions(&state).await, vec!["user_invite"]);

        let dup = create_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Json(CreateUserRequest {
                email: "NEW@example.com".into(),
                role: Role::Viewer,
            }),
        )
        .await;
        assert!(dup.is_err(), "emails are unique, case-insensitively");
        let bad = create_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Json(CreateUserRequest {
                email: "not-an-email".into(),
                role: Role::Viewer,
            }),
        )
        .await;
        assert!(bad.is_err());
    }

    #[tokio::test]
    async fn role_changes_and_disabling_sign_the_user_out() {
        let state = test_state().await;
        seed(&state, "admin", "admin").await;
        seed(&state, "dev", "deployer").await;
        let response = update_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("dev".into()),
            Json(UpdateUserRequest {
                role: Some(Role::Viewer),
                disabled: Some(true),
            }),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        let body = json(response).await;
        assert_eq!(body["data"]["role"], "viewer");
        assert_eq!(body["data"]["disabled"], true);
        let sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = 'dev'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(sessions, 0);
        assert_eq!(
            audit_actions(&state).await,
            vec!["user_role_change", "user_disable", "session_revoke"]
        );
    }

    #[tokio::test]
    async fn admins_cant_lock_themselves_or_the_server_out() {
        let state = test_state().await;
        seed(&state, "admin", "admin").await;
        let own = update_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("admin".into()),
            Json(UpdateUserRequest {
                role: Some(Role::Viewer),
                disabled: None,
            }),
        )
        .await;
        assert!(own.is_err(), "no self-demotion");
        let own_delete = delete_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("admin".into()),
        )
        .await;
        assert!(own_delete.is_err(), "no self-deletion");

        // The local socket has no user: it may act on the admin, but not remove the last one.
        seed(&state, "dev", "deployer").await;
        let last = delete_user(
            RateLimit,
            Authz::unchecked(Principal::local_socket()),
            State(state.clone()),
            Path("admin".into()),
        )
        .await;
        let err = last
            .err()
            .unwrap_or_else(|| panic!("the last admin can't be deleted"));
        assert!(err.0.to_string().contains("only active admin"));

        // With a second admin, the first can be removed.
        seed(&state, "admin2", "admin").await;
        assert!(
            delete_user(
                RateLimit,
                Authz::unchecked(Principal::local_socket()),
                State(state.clone()),
                Path("admin".into()),
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn deleting_a_user_deletes_their_tokens() {
        let state = test_state().await;
        seed(&state, "admin", "admin").await;
        seed(&state, "dev", "deployer").await;
        sqlx::query("INSERT INTO api_tokens (id, name, prefix, token_hash, owner_user_id, kind, scopes, created_at) VALUES ('t', 'ci', 'p', 'h', 'dev', 'ci', '[]', 'x')")
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        delete_user(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("dev".into()),
        )
        .await
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn admins_can_list_and_revoke_another_users_sessions() {
        let state = test_state().await;
        seed(&state, "admin", "admin").await;
        seed(&state, "dev", "deployer").await;
        let list = list_users(RateLimit, admin(&state).await, State(state.clone()))
            .await
            .map(IntoResponse::into_response)
            .unwrap_or_else(|e| panic!("{:?}", e.0));
        let body = json(list).await;
        let dev = body["data"]
            .as_array()
            .and_then(|a| a.iter().find(|u| u["id"] == "dev").cloned())
            .unwrap_or_default();
        assert_eq!(dev["active_sessions"], 1);
        let me = body["data"]
            .as_array()
            .and_then(|a| a.iter().find(|u| u["id"] == "admin").cloned())
            .unwrap_or_default();
        assert_eq!(me["is_you"], true);

        revoke_user_sessions(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("dev".into()),
        )
        .await
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        let sessions = list_user_sessions(
            RateLimit,
            admin(&state).await,
            State(state.clone()),
            Path("dev".into()),
        )
        .await
        .map(IntoResponse::into_response)
        .unwrap_or_else(|e| panic!("{:?}", e.0));
        assert_eq!(json(sessions).await["data"], serde_json::json!([]));
    }
}
