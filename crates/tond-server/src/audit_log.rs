// audit_log.rs — Phase 5.5: Audit log write helper + AuthActor extractor
//
// Public surface:
//
//   write_audit_log(db, app_id, app_name, action, actor, detail)
//     Fire-and-forget INSERT into the audit_log table. Non-critical: the
//     result is discarded with `let _ = ...` so a log write failure never
//     surfaces as a handler error.
//
//   AuthActor — Axum extractor that resolves the caller's identity string:
//     - Session cookie  → user email (e.g. "admin@example.com")
//     - Bearer API key  → "api-key"
//     - Neither         → "unknown"   (should not happen behind RequireAuth)

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    handlers::auth::{extract_session_token, validate_session},
    state::AppState,
};

/// Write a single audit log entry.  Non-fatal: the result is intentionally
/// discarded by callers — a logging failure must not cause a handler error.
pub async fn write_audit_log(
    db: &SqlitePool,
    app_id: Option<&str>,
    app_name: Option<&str>,
    action: &str,
    actor: &str,
    detail: Option<&str>,
) {
    let id = Uuid::new_v4().to_string();
    let _ = sqlx::query(
        "INSERT INTO audit_log (id, app_id, app_name, action, actor, detail)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(app_id)
    .bind(app_name)
    .bind(action)
    .bind(actor)
    .bind(detail)
    .execute(db)
    .await;
}

/// Axum extractor: resolves the authenticated caller's identity as a string.
///
/// This is a *non-rejecting* extractor — it always succeeds, falling back to
/// "unknown" if no identity can be determined.  Use it alongside `RequireAuth`
/// (which performs the actual access control) to obtain the actor label for
/// audit log entries.
pub struct AuthActor(pub String);

impl FromRequestParts<AppState> for AuthActor {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Try session cookie first (most common in dashboard usage)
        if let Some(token) = extract_session_token(&parts.headers)
            && let Ok(Some(user)) = validate_session(&state.db, &token).await
        {
            return Ok(AuthActor(user.email));
        }

        // Fall back to bearer token label
        if let Some(auth_val) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            && auth_val.starts_with("Bearer ")
        {
            return Ok(AuthActor("api-key".to_string()));
        }

        Ok(AuthActor("unknown".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::{HeaderValue, header, request::Parts};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for audit log tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_user_and_session(state: &AppState) {
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled)
             VALUES (?, ?, ?, ?, datetime('now'), datetime('now'), NULL, 0)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hashed")
        .bind("admin")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, user_agent, ip_address, last_used_at)
             VALUES (?, ?, datetime('now'), datetime('now', '+1 day'), ?, ?, datetime('now'))",
        )
        .bind(tond_common::hash_key("session-1"))
        .bind("user-1")
        .bind("test-agent")
        .bind("127.0.0.1")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session: {err}"));
    }

    fn empty_parts() -> Parts {
        axum::http::Request::new(axum::body::Body::empty())
            .into_parts()
            .0
    }

    #[tokio::test]
    async fn write_audit_log_persists_rows_without_failing_callers() {
        let state = test_state().await;

        write_audit_log(
            &state.db,
            Some("app-1"),
            Some("frontend"),
            "deploy",
            "admin@example.com",
            Some("manual deploy"),
        )
        .await;

        let row: (String, String, String, String, String) =
            sqlx::query_as("SELECT app_id, app_name, action, actor, detail FROM audit_log")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to load audit log row: {err}"));

        assert_eq!(
            row,
            (
                "app-1".to_string(),
                "frontend".to_string(),
                "deploy".to_string(),
                "admin@example.com".to_string(),
                "manual deploy".to_string(),
            ),
            "write_audit_log should persist the supplied audit metadata"
        );
    }

    #[tokio::test]
    async fn auth_actor_prefers_session_then_bearer_then_unknown() {
        let state = test_state().await;
        seed_user_and_session(&state).await;

        let mut parts = empty_parts();
        parts.headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-tond_session=session-1"),
        );
        parts.headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer some-api-key"),
        );
        let actor = AuthActor::from_request_parts(&mut parts, &state)
            .await
            .unwrap_or_else(|_| panic!("AuthActor extraction should never reject"));
        assert_eq!(
            actor.0, "admin@example.com",
            "AuthActor should prefer the authenticated session email over a bearer token label"
        );

        let mut parts = empty_parts();
        parts.headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer some-api-key"),
        );
        let actor = AuthActor::from_request_parts(&mut parts, &state)
            .await
            .unwrap_or_else(|_| panic!("AuthActor extraction should never reject"));
        assert_eq!(
            actor.0, "api-key",
            "AuthActor should fall back to the bearer token label when no valid session cookie is present"
        );

        let mut parts = empty_parts();
        let actor = AuthActor::from_request_parts(&mut parts, &state)
            .await
            .unwrap_or_else(|_| panic!("AuthActor extraction should never reject"));
        assert_eq!(
            actor.0, "unknown",
            "AuthActor should fall back to 'unknown' when no identity can be determined"
        );
    }
}
