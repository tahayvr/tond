// handlers/auth/password_links.rs — one-time set-password links
//
// Admins never choose other people's passwords. Inviting a user (or resetting
// their password) creates a random link token valid for 24 hours; only its
// SHA-256 is stored. Opening the link lets the user pick a password under the
// normal policy. Creating a new link for a user replaces any older one, and a
// link is consumed atomically so it works exactly once.

use chrono::{Duration, Utc};
use sqlx::SqlitePool;
use tond_common::{generate_raw_key, hash_key};

/// How long a set-password link stays valid.
pub const LINK_TTL_HOURS: i64 = 24;

/// Shown for unknown, used or expired links.
pub const INVALID_LINK: &str = "This link is invalid, has expired or was already used. Ask an admin of this server for a new one.";

/// Why the link was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPurpose {
    Invite,
    Reset,
}

impl LinkPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkPurpose::Invite => "invite",
            LinkPurpose::Reset => "reset",
        }
    }
}

/// A live link and the account it belongs to.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkInfo {
    pub user_id: String,
    pub email: String,
    pub purpose: String,
    pub expires_at: String,
}

/// Create a link for `user_id`, replacing older ones. Returns (token, expires_at).
pub async fn create(
    db: &SqlitePool,
    user_id: &str,
    purpose: LinkPurpose,
) -> Result<(String, String), sqlx::Error> {
    let token = generate_raw_key();
    let now = Utc::now();
    let expires_at = (now + Duration::hours(LINK_TTL_HOURS)).to_rfc3339();
    let mut tx = crate::db::begin_write(db).await?;
    sqlx::query("DELETE FROM password_links WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO password_links (token_hash, user_id, purpose, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(hash_key(&token))
    .bind(user_id)
    .bind(purpose.as_str())
    .bind(&expires_at)
    .bind(now.to_rfc3339())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((token, expires_at))
}

/// Look up a live link without using it.
pub async fn lookup(db: &SqlitePool, token: &str) -> Result<Option<LinkInfo>, sqlx::Error> {
    sqlx::query_as::<_, LinkInfo>(
        "SELECT l.user_id, u.email, l.purpose, l.expires_at
         FROM password_links l JOIN users u ON u.id = l.user_id
         WHERE l.token_hash = ? AND l.expires_at > ?",
    )
    .bind(hash_key(token.trim()))
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(db)
    .await
}

/// Use a link (single use): deletes it and returns what it was for.
pub async fn consume(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    token: &str,
) -> Result<Option<LinkInfo>, sqlx::Error> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "DELETE FROM password_links WHERE token_hash = ? AND expires_at > ?
         RETURNING user_id, purpose, expires_at",
    )
    .bind(hash_key(token.trim()))
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((user_id, purpose, expires_at)) = row else {
        return Ok(None);
    };
    let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Some(LinkInfo {
        user_id,
        email,
        purpose,
        expires_at,
    }))
}

/// The dashboard path for a link token.
pub fn link_path(token: &str) -> String {
    format!("/set-password?token={token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_with_user() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES ('u1', 'new@example.com', '', 'viewer', 'x', 'x')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed: {err}"));
        db
    }

    #[tokio::test]
    async fn links_work_once_and_only_the_newest_one_counts() {
        let db = pool_with_user().await;
        let (old, _) = create(&db, "u1", LinkPurpose::Invite)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let (token, _) = create(&db, "u1", LinkPurpose::Reset)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            lookup(&db, &old)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
        let info = lookup(&db, &token)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("the newest link is live"));
        assert_eq!(info.email, "new@example.com");
        assert_eq!(info.purpose, "reset");

        let mut tx = crate::db::begin_write(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            consume(&mut tx, &token)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_some()
        );
        tx.commit().await.unwrap_or_else(|e| panic!("{e}"));
        let mut tx = crate::db::begin_write(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            consume(&mut tx, &token)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none(),
            "links are single-use"
        );
    }

    #[tokio::test]
    async fn expired_links_are_refused() {
        let db = pool_with_user().await;
        let (token, _) = create(&db, "u1", LinkPurpose::Invite)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let _ = sqlx::query("UPDATE password_links SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&db)
            .await;
        assert!(
            lookup(&db, &token)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn link_paths_point_at_the_dashboard_page() {
        assert_eq!(link_path("abc"), "/set-password?token=abc");
    }
}
