// handlers/auth/session.rs — public session utilities used by RequireAuth extractor
//
// Session cookies carry a random 256-bit token; the sessions table stores only
// its SHA-256 hash (`tond_common::hash_key`), so a leaked database or backup
// can't be replayed as a login. The same applies to pending 2FA logins.

use axum::http::{HeaderMap, header};
use chrono::Utc;
use tond_common::{User, hash_key};

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

/// Validate a session token and return the associated user.
pub async fn validate_session(
    pool: &sqlx::SqlitePool,
    token: &str,
) -> Result<Option<User>, sqlx::Error> {
    let now = Utc::now().to_rfc3339();

    let user = sqlx::query_as::<_, User>(
        "SELECT u.* FROM users u
         JOIN sessions s ON s.user_id = u.id
         WHERE s.id = ? AND s.expires_at > ?",
    )
    .bind(hash_key(token))
    .bind(&now)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};
    use sqlx::SqlitePool;

    use super::*;

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
        .bind("2026-01-01T00:00:00Z")
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
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, 'user-1', ?, '2026-01-01T00:00:00Z')",
        )
        .bind(&stored)
        .bind(&future)
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
