// handlers/auth/session.rs — public session utilities used by RequireAuth extractor

use axum::http::{HeaderMap, header};
use chrono::Utc;
use tond_common::User;

/// Extract the session token from the Cookie header.
pub fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                let c = c.trim();
                if let Some(val) = c.strip_prefix("tond_session=") {
                    if val.is_empty() {
                        None
                    } else {
                        Some(val.to_string())
                    }
                } else {
                    None
                }
            })
        })
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
    .bind(token)
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
            HeaderValue::from_static("foo=bar; tond_session=session-token; theme=dark"),
        );

        let token = extract_session_token(&headers);

        assert_eq!(
            token.as_deref(),
            Some("session-token"),
            "extract_session_token should extract the tond_session cookie from a combined Cookie header"
        );
    }

    #[test]
    fn extract_session_token_rejects_missing_or_empty_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("tond_session="));
        assert_eq!(
            extract_session_token(&headers),
            None,
            "extract_session_token should ignore empty tond_session cookie values"
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
        .bind("session-1")
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
        .bind("expired-session")
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
}
