// handlers/auth/session.rs — public session utilities used by principal.rs
//
// Session cookies carry a random 256-bit token; the sessions table stores only
// its SHA-256 hash (`tond_common::hash_key`), so a leaked database or backup
// can't be replayed as a login. The same applies to pending 2FA logins.
//
// Lifetime: a session ends after `session_idle_timeout_hours` without a
// request (default 12 h) and `session_max_age_days` after sign-in however
// active it is (default 30 days, stored as `expires_at`). Both come from the
// settings cache. `last_used_at` is refreshed at most once a minute so busy
// dashboards don't write on every request. Sessions of disabled users are
// refused (and deleted when the account is disabled).

use axum::http::{HeaderMap, header};
use chrono::{Duration, SecondsFormat, Utc};
use tond_common::{User, hash_key};

use crate::state::AppState;

/// Name of the dashboard session cookie.
///
/// The `__Host-` prefix makes browsers accept the cookie only with `Secure`,
/// `Path=/` and no `Domain`, so a sibling subdomain can't set or shadow it.
pub const SESSION_COOKIE: &str = "__Host-tond_session";

/// Name of the short-lived cookie that marks a login waiting for its 2FA code.
pub const MFA_PENDING_COOKIE: &str = "__Host-tond_mfa";

/// Read a cookie value by name from the Cookie header (empty values are ignored).
pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|c| {
            let (key, value) = c.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_string())
        })
}

/// Extract the session token from the Cookie header.
pub fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, SESSION_COOKIE)
}

/// How often `last_used_at` is refreshed.
const TOUCH_INTERVAL_SECS: i64 = 60;

/// Validate a session token and return the associated user.
pub async fn validate_session(state: &AppState, token: &str) -> Result<Option<User>, sqlx::Error> {
    Ok(validate_session_with_id(state, token)
        .await?
        .map(|(user, _)| user))
}

/// Validate a session token; returns the user and the session's stored id.
pub async fn validate_session_with_id(
    state: &AppState,
    token: &str,
) -> Result<Option<(User, String)>, sqlx::Error> {
    let idle_hours = state.settings.read().await.session_idle_timeout_hours;
    validate_session_in(&state.db, token, idle_hours).await
}

/// `validate_session_with_id` with an explicit idle timeout (testable).
pub async fn validate_session_in(
    pool: &sqlx::SqlitePool,
    token: &str,
    idle_hours: i64,
) -> Result<Option<(User, String)>, sqlx::Error> {
    let now = Utc::now();
    let id = hash_key(token);
    let idle_cutoff = (now - Duration::hours(idle_hours.max(1))).to_rfc3339();

    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT s.last_used_at FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.id = ? AND s.expires_at > ? AND u.disabled_at IS NULL
           AND COALESCE(s.last_used_at, s.created_at) > ?",
    )
    .bind(&id)
    .bind(now.to_rfc3339())
    .bind(&idle_cutoff)
    .fetch_optional(pool)
    .await?;
    let Some((last_used,)) = row else {
        return Ok(None);
    };

    let stale = last_used
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_none_or(|t| now.signed_duration_since(t) > Duration::seconds(TOUCH_INTERVAL_SECS));
    if stale {
        let _ = sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(now.to_rfc3339_opts(SecondsFormat::Secs, true))
            .bind(&id)
            .execute(pool)
            .await;
    }

    let user = sqlx::query_as::<_, User>(
        "SELECT u.* FROM users u JOIN sessions s ON s.user_id = u.id WHERE s.id = ?",
    )
    .bind(&id)
    .fetch_optional(pool)
    .await?;
    Ok(user.map(|u| (u, id)))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};
    use sqlx::SqlitePool;

    use super::*;

    async fn validate_session(pool: &SqlitePool, token: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(validate_session_in(pool, token, 12).await?.map(|(u, _)| u))
    }

    async fn seed(pool: &SqlitePool, token: &str, created: &str, last_used: Option<&str>) {
        let now = Utc::now();
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO users (id, email, password_hash, role, created_at, updated_at) VALUES ('u', 'u@example.com', 'h', 'viewer', ?, ?)",
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(pool)
        .await;
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at, last_used_at) VALUES (?, 'u', ?, ?, ?)",
        )
        .bind(hash_key(token))
        .bind((now + chrono::Duration::days(30)).to_rfc3339())
        .bind(created)
        .bind(last_used)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("seed session: {err}"));
    }

    #[tokio::test]
    async fn idle_sessions_end_and_active_ones_are_touched() {
        let pool = test_pool().await;
        let now = Utc::now();
        let thirteen_hours_ago = (now - chrono::Duration::hours(13)).to_rfc3339();
        let two_hours_ago = (now - chrono::Duration::hours(2)).to_rfc3339();
        seed(
            &pool,
            "idle",
            &thirteen_hours_ago,
            Some(&thirteen_hours_ago),
        )
        .await;
        seed(&pool, "active", &thirteen_hours_ago, Some(&two_hours_ago)).await;

        assert!(
            validate_session(&pool, "idle")
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none(),
            "a session idle for longer than the timeout must end"
        );
        let (_, id) = validate_session_in(&pool, "active", 12)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("an active session stays valid"));
        let touched: String = sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(touched > two_hours_ago, "use refreshes last_used_at");
        assert!(
            validate_session_in(&pool, "idle", 24)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_some(),
            "the idle timeout is configurable"
        );
    }

    #[tokio::test]
    async fn sessions_of_disabled_users_are_refused() {
        let pool = test_pool().await;
        seed(&pool, "s", &Utc::now().to_rfc3339(), None).await;
        assert!(
            validate_session(&pool, "s")
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_some()
        );
        let _ = sqlx::query("UPDATE users SET disabled_at = 'now' WHERE id = 'u'")
            .execute(&pool)
            .await;
        assert!(
            validate_session(&pool, "s")
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for session tests: {err}"));
        pool
    }

    #[test]
    fn extract_session_token_reads_tond_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("foo=bar; __Host-tond_session=session-token; theme=dark"),
        );

        let token = extract_session_token(&headers);

        assert_eq!(
            token.as_deref(),
            Some("session-token"),
            "extract_session_token should extract the session cookie from a combined Cookie header"
        );
    }

    #[test]
    fn extract_session_token_rejects_missing_or_empty_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-tond_session="),
        );
        assert_eq!(
            extract_session_token(&headers),
            None,
            "extract_session_token should ignore empty session cookie values"
        );

        let headers = HeaderMap::new();
        assert_eq!(
            extract_session_token(&headers),
            None,
            "extract_session_token should return None when no Cookie header is present"
        );
    }

    #[tokio::test]
    async fn validate_session_returns_user_for_non_expired_session() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hash")
        .bind("admin")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(0)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user for session validation test: {err}"));
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at, user_agent, ip_address, last_used_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(hash_key("session-1"))
        .bind("user-1")
        .bind(&future)
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed active session: {err}"));

        let user = validate_session(&pool, "session-1")
            .await
            .unwrap_or_else(|err| {
                panic!("validate_session should succeed for active sessions: {err}")
            });

        assert_eq!(
            user.map(|u| u.email),
            Some("admin@example.com".to_string()),
            "validate_session should return the joined user for a non-expired session"
        );
    }

    #[tokio::test]
    async fn validate_session_returns_none_for_expired_or_unknown_session() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hash")
        .bind("admin")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(0)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user for expired session test: {err}"));
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at, user_agent, ip_address, last_used_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(hash_key("expired-session"))
        .bind("user-1")
        .bind(&past)
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind("2026-01-01T00:00:00Z")
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed expired session: {err}"));

        assert!(
            validate_session(&pool, "expired-session")
                .await
                .unwrap_or_else(|err| panic!(
                    "validate_session should not error for expired sessions: {err}"
                ))
                .is_none(),
            "validate_session should reject expired sessions"
        );
        assert!(
            validate_session(&pool, "missing-session")
                .await
                .unwrap_or_else(|err| panic!(
                    "validate_session should not error for missing sessions: {err}"
                ))
                .is_none(),
            "validate_session should return None for unknown sessions"
        );
    }

    #[test]
    fn extract_session_token_ignores_legacy_and_lookalike_cookies() {
        // A cookie without the __Host- prefix can be planted by a sibling subdomain;
        // it must never be read as the session.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("tond_session=planted; x__Host-tond_session=planted"),
        );
        assert_eq!(extract_session_token(&headers), None);
    }

    #[tokio::test]
    async fn validate_session_rejects_the_stored_hash_as_a_token() {
        // The DB holds only hash(token): presenting the stored value must not work.
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES ('user-1', 'a@example.com', 'hash', 'admin', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let stored = hash_key("real-token");
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, 'user-1', ?, ?)",
        )
        .bind(&stored)
        .bind(&future)
        .bind(Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session: {err}"));

        let by_hash = validate_session(&pool, &stored)
            .await
            .unwrap_or_else(|err| panic!("validate_session failed: {err}"));
        assert!(
            by_hash.is_none(),
            "the stored hash must not be usable as a session token"
        );
        let by_token = validate_session(&pool, "real-token")
            .await
            .unwrap_or_else(|err| panic!("validate_session failed: {err}"));
        assert!(
            by_token.is_some(),
            "the raw token should validate against its stored hash"
        );
    }
}
