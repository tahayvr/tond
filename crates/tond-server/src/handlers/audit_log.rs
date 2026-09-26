// handlers/audit_log.rs — audit log queries and chain verification
//
// Routes (both need `events:read`):
//   GET /api/v1/audit-log?actor=&action=&resource_type=&resource_id=&q=&outcome=&before=&limit=
//     Newest first, filtered in SQL, paginated by sequence number: pass the
//     response's `next_before` as `before` for the next (older) page.
//       actor         — exact actor label (email, token name, "local-socket")
//       action        — exact action, or a prefix ending in '*' (e.g. "login*")
//       resource_type — "app", "user", "token", …; resource_id narrows to one
//       q             — case-insensitive substring of actor, action, resource or detail
//       outcome       — "success" | "failure"
//       limit         — 1–200 (default 50)
//   GET /api/v1/audit-log/verify — walk the hash chain (see audit_log.rs)

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use sqlx::QueryBuilder;
use tond_common::*;

use crate::{
    audit_log,
    auth::{Authz, RateLimit, scope},
    error::AppError,
    state::AppState,
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct AuditLogQuery {
    pub actor: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub q: Option<String>,
    pub outcome: Option<String>,
    pub before: Option<i64>,
    pub limit: Option<i64>,
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Escape LIKE wildcards in user input.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Run the filtered, paginated query.
pub async fn query_page(
    db: &sqlx::SqlitePool,
    q: &AuditLogQuery,
) -> Result<AuditLogPage, sqlx::Error> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut sql = QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM audit_log WHERE 1 = 1");
    if let Some(before) = q.before {
        sql.push(" AND seq < ").push_bind(before);
    }
    if let Some(actor) = non_empty(&q.actor) {
        sql.push(" AND actor_label = ").push_bind(actor.to_string());
    }
    if let Some(action) = non_empty(&q.action) {
        match action.strip_suffix('*') {
            Some(prefix) => {
                sql.push(" AND action LIKE ")
                    .push_bind(format!("{}%", like_escape(prefix)))
                    .push(" ESCAPE '\\'");
            }
            None => {
                sql.push(" AND action = ").push_bind(action.to_string());
            }
        }
    }
    if let Some(kind) = non_empty(&q.resource_type) {
        sql.push(" AND resource_type = ")
            .push_bind(kind.to_string());
    }
    if let Some(id) = non_empty(&q.resource_id) {
        sql.push(" AND resource_id = ").push_bind(id.to_string());
    }
    if let Some(outcome) = non_empty(&q.outcome) {
        sql.push(" AND outcome = ").push_bind(outcome.to_string());
    }
    if let Some(text) = non_empty(&q.q) {
        let pattern = format!("%{}%", like_escape(&text.to_lowercase()));
        sql.push(" AND (");
        for (i, col) in ["actor_label", "action", "resource_name", "detail"]
            .iter()
            .enumerate()
        {
            if i > 0 {
                sql.push(" OR ");
            }
            sql.push(format!("lower(COALESCE({col}, '')) LIKE "))
                .push_bind(pattern.clone())
                .push(" ESCAPE '\\'");
        }
        sql.push(")");
    }
    // One extra row tells us whether there is another page.
    sql.push(" ORDER BY seq DESC LIMIT ").push_bind(limit + 1);

    let mut rows: Vec<AuditLog> = sql.build_query_as().fetch_all(db).await?;
    let next_before = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| r.seq)
    } else {
        None
    };
    Ok(AuditLogPage {
        entries: rows.into_iter().map(AuditLogResponse::from).collect(),
        next_before,
    })
}

pub async fn list_audit_log(
    _rate: RateLimit,
    _auth: Authz<scope::EventsRead>,
    State(state): State<AppState>,
    Query(query): Query<AuditLogQuery>,
) -> Result<impl IntoResponse, AppError> {
    let page = query_page(&state.db, &query).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(page))))
}

pub async fn verify_audit_log(
    _rate: RateLimit,
    _auth: Authz<scope::EventsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let report = audit_log::verify(&state.db).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(report))))
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::*;
    use crate::audit_log::{AuditActor, AuditEntry, record};

    async fn seeded() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        let alice = AuditActor::user("u1", "alice@example.com", None);
        let bob = AuditActor::user("u2", "bob@example.com", None);
        for i in 0..60 {
            record(
                &db,
                &alice,
                AuditEntry::new("deploy")
                    .app("a1", "web")
                    .detail(format!("n={i}")),
            )
            .await;
        }
        record(&db, &bob, AuditEntry::new("login")).await;
        record(&db, &bob, AuditEntry::new("login_failed").failed()).await;
        record(
            &db,
            &AuditActor::local_socket(),
            AuditEntry::new("token_create").resource("token", "t1", "ci"),
        )
        .await;
        db
    }

    #[tokio::test]
    async fn filters_apply_before_pagination() {
        // The old endpoint filtered after a 50-row limit; bob's events sit
        // behind 60 of alice's and must still be found.
        let db = seeded().await;
        let page = query_page(
            &db,
            &AuditLogQuery {
                actor: Some("bob@example.com".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(page.entries.len(), 2);
        assert!(page.next_before.is_none());

        let logins = query_page(
            &db,
            &AuditLogQuery {
                action: Some("login*".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(logins.entries.len(), 2);

        let failures = query_page(
            &db,
            &AuditLogQuery {
                outcome: Some("failure".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(failures.entries.len(), 1);

        let tokens = query_page(
            &db,
            &AuditLogQuery {
                resource_type: Some("token".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tokens.entries[0].actor_label, "local-socket");
        assert_eq!(tokens.entries[0].actor_kind, "local_socket");
    }

    #[tokio::test]
    async fn pages_walk_backwards_by_sequence() {
        let db = seeded().await;
        let first = query_page(&db, &AuditLogQuery::default())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(first.entries.len(), 50);
        assert_eq!(first.entries[0].seq, 63, "newest first");
        let before = first.next_before.unwrap_or_else(|| panic!("more pages"));
        let second = query_page(
            &db,
            &AuditLogQuery {
                before: Some(before),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(second.entries.len(), 13);
        assert!(second.next_before.is_none());
        assert_eq!(second.entries[0].seq, before - 1);
    }

    #[tokio::test]
    async fn text_search_matches_details_and_escapes_wildcards() {
        let db = seeded().await;
        let hit = query_page(
            &db,
            &AuditLogQuery {
                q: Some("N=59".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(hit.entries.len(), 1);
        let wildcard = query_page(
            &db,
            &AuditLogQuery {
                q: Some("%".into()),
                ..AuditLogQuery::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            wildcard.entries.is_empty(),
            "'%' is literal, not a wildcard"
        );
    }
}
