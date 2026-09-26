// idempotency.rs — safe retries with the Idempotency-Key header
//
// A mutation (POST/PUT/PATCH/DELETE under /api/v1, see `applies`) sent with
// `Idempotency-Key: <key>` is recorded before it runs. For the next 24 h a
// repeat with the same key, from the same caller, to the same method and path:
//   - same body, first one finished with 2xx → the stored response is
//     replayed (with `Idempotent-Replayed: true`); nothing runs again
//   - same body, first one still running     → 409 IDEMPOTENCY_KEY_IN_USE
//   - different body                          → 422 IDEMPOTENCY_KEY_REUSED
// Anything other than a 2xx isn't stored, so fixing the cause and retrying
// with the same key works. Without the header nothing changes.
//
// The caller is authenticated here (the handler authenticates again) so
// that unauthenticated requests never write a row. Stored bodies are
// encrypted (Purpose::IdempotentResponse): a replayed `POST /tokens` holds a
// token secret. The table stays small: bodies over MAX_STORED_BODY aren't
// stored, rows expire after TTL_SECS, and there are at most MAX_ROWS (pruned
// on insert every PRUNE_EVERY inserts and by the janitor).
//
// Excluded: sign-in and sessions (/auth: Set-Cookie must never be replayed),
// forge webhooks (they dedupe by delivery id), terminal tickets (one-time),
// the GitHub App flows, and uploads (their bodies are too large to hash).

use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tond_common::{ApiErrorBody, code};

use crate::auth::principal::{Actor, Principal, authenticate};
use crate::error::error_response;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

pub const HEADER: &str = "idempotency-key";
pub const REPLAYED_HEADER: &str = "idempotent-replayed";
/// How long a key is remembered.
pub const TTL_SECS: i64 = 24 * 3600;
/// A request still marked running after this long is treated as abandoned
/// (the server stopped mid-request) and may be retried.
const STALE_RUNNING_SECS: i64 = 15 * 60;
/// Longest key accepted.
const MAX_KEY_LEN: usize = 255;
/// Request bodies read to hash them (axum's default JSON limit).
const MAX_REQUEST_BODY: usize = 2 * 1024 * 1024;
/// Responses larger than this are passed through without being stored.
pub const MAX_STORED_BODY: usize = 32 * 1024;
/// Largest response body buffered at all.
const MAX_RESPONSE_BODY: usize = 16 * 1024 * 1024;
/// Upper bound on stored keys.
pub const MAX_ROWS: i64 = 2000;
const PRUNE_EVERY: u64 = 64;

static INSERTS: AtomicU64 = AtomicU64::new(0);

/// Whether mutations on this route template honour Idempotency-Key.
pub fn applies(method: &str, template: &str) -> bool {
    const EXCLUDED_PREFIXES: &[&str] = &["/api/v1/auth/", "/api/v1/webhooks/"];
    const EXCLUDED: &[&str] = &[
        "/api/v1/terminal/tickets",
        "/api/v1/apps/{id}/upload",
        "/api/v1/github-apps/manifest",
        "/api/v1/github-apps/complete",
    ];
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
        && template.starts_with("/api/v1/")
        && !EXCLUDED_PREFIXES.iter().any(|p| template.starts_with(p))
        && !EXCLUDED.contains(&template)
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    let hint = tond_common::error_code_info(code).and_then(|c| c.hint);
    let mut body = ApiErrorBody::new(code, message);
    body.hint = hint.map(str::to_string);
    if code == code::INVALID_INPUT {
        body.field = Some("Idempotency-Key".to_string());
    }
    error_response(status, body)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Who a stored key belongs to.
fn actor_key(principal: &Principal) -> String {
    match principal.actor() {
        Actor::User { id, .. } => format!("user:{id}"),
        Actor::Token { id, .. } => format!("token:{id}"),
        Actor::LocalSocket => "local".to_string(),
    }
}

/// A valid key: 1–255 visible ASCII characters.
fn parse_key(headers: &HeaderMap) -> Option<Result<String, ()>> {
    let raw = headers.get(HEADER)?;
    let Ok(s) = raw.to_str() else {
        return Some(Err(()));
    };
    let s = s.trim();
    if s.is_empty() || s.len() > MAX_KEY_LEN || !s.bytes().all(|b| b.is_ascii_graphic()) {
        return Some(Err(()));
    }
    Some(Ok(s.to_string()))
}

fn request_hash(method: &Method, path_and_query: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_str().as_bytes());
    h.update(b"\n");
    h.update(path_and_query.as_bytes());
    h.update(b"\n");
    h.update(body);
    hex::encode(h.finalize())
}

/// What the store says about a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// New (or expired) key: run the request.
    Run,
    Replay {
        status: u16,
        content_type: Option<String>,
        body: String,
    },
    InUse,
    Mismatch,
}

/// request_hash, status, content_type, response, created_at
type StoredKey = (String, Option<i64>, Option<String>, Option<String>, i64);

/// The identity of one keyed request.
#[derive(Debug, Clone)]
pub struct KeyedRequest {
    pub actor: String,
    pub method: String,
    pub path: String,
    pub key: String,
    pub hash: String,
}

/// Record the key as running, or report what's stored for it.
pub async fn claim(db: &SqlitePool, req: &KeyedRequest, now: i64) -> sqlx::Result<Claim> {
    let inserted = sqlx::query(
        "INSERT INTO idempotency_keys (actor, method, path, key, request_hash, created_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (actor, method, path, key) DO NOTHING",
    )
    .bind(&req.actor)
    .bind(&req.method)
    .bind(&req.path)
    .bind(&req.key)
    .bind(&req.hash)
    .bind(now)
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 1 {
        if INSERTS
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(PRUNE_EVERY)
        {
            let _ = prune(db, now).await;
        }
        return Ok(Claim::Run);
    }
    let row: Option<StoredKey> = sqlx::query_as(
        "SELECT request_hash, status, content_type, response, created_at FROM idempotency_keys
         WHERE actor = ? AND method = ? AND path = ? AND key = ?",
    )
    .bind(&req.actor)
    .bind(&req.method)
    .bind(&req.path)
    .bind(&req.key)
    .fetch_optional(db)
    .await?;
    let Some((hash, status, content_type, response, created_at)) = row else {
        // Deleted between the two statements: try once more from scratch.
        return Box::pin(claim(db, req, now)).await;
    };
    let expired = created_at < now - TTL_SECS;
    let abandoned = status.is_none() && created_at < now - STALE_RUNNING_SECS;
    if expired || abandoned {
        sqlx::query(
            "UPDATE idempotency_keys
             SET request_hash = ?, status = NULL, content_type = NULL, response = NULL, created_at = ?
             WHERE actor = ? AND method = ? AND path = ? AND key = ? AND created_at = ?",
        )
        .bind(&req.hash)
        .bind(now)
        .bind(&req.actor)
        .bind(&req.method)
        .bind(&req.path)
        .bind(&req.key)
        .bind(created_at)
        .execute(db)
        .await?;
        return Ok(Claim::Run);
    }
    if hash != req.hash {
        return Ok(Claim::Mismatch);
    }
    let (Some(status), Some(response)) = (status, response) else {
        return Ok(Claim::InUse);
    };
    match secret_key::decrypt(Purpose::IdempotentResponse, &response) {
        Ok(body) => Ok(Claim::Replay {
            status: u16::try_from(status).unwrap_or(200),
            content_type,
            body,
        }),
        // Unreadable (e.g. after a key rotation): forget it and run again.
        Err(_) => {
            release(db, req).await?;
            Box::pin(claim(db, req, now)).await
        }
    }
}

/// Store the finished response for replays.
pub async fn complete(
    db: &SqlitePool,
    req: &KeyedRequest,
    status: u16,
    content_type: Option<&str>,
    body: &str,
) -> anyhow::Result<()> {
    let sealed = secret_key::encrypt(Purpose::IdempotentResponse, body)?;
    sqlx::query(
        "UPDATE idempotency_keys SET status = ?, content_type = ?, response = ?
         WHERE actor = ? AND method = ? AND path = ? AND key = ?",
    )
    .bind(i64::from(status))
    .bind(content_type)
    .bind(sealed)
    .bind(&req.actor)
    .bind(&req.method)
    .bind(&req.path)
    .bind(&req.key)
    .execute(db)
    .await?;
    Ok(())
}

/// Forget a key (the request failed, or its response can't be stored).
pub async fn release(db: &SqlitePool, req: &KeyedRequest) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM idempotency_keys WHERE actor = ? AND method = ? AND path = ? AND key = ?",
    )
    .bind(&req.actor)
    .bind(&req.method)
    .bind(&req.path)
    .bind(&req.key)
    .execute(db)
    .await?;
    Ok(())
}

/// Drop expired keys and keep at most MAX_ROWS. Returns rows deleted.
pub async fn prune(db: &SqlitePool, now: i64) -> sqlx::Result<u64> {
    let expired = sqlx::query("DELETE FROM idempotency_keys WHERE created_at < ?")
        .bind(now - TTL_SECS)
        .execute(db)
        .await?
        .rows_affected();
    let over = sqlx::query(
        "DELETE FROM idempotency_keys WHERE created_at < (
            SELECT created_at FROM idempotency_keys ORDER BY created_at DESC LIMIT 1 OFFSET ?)",
    )
    .bind(MAX_ROWS)
    .execute(db)
    .await?
    .rows_affected();
    Ok(expired + over)
}

fn replay(status: u16, content_type: Option<String>, body: String) -> Response {
    let mut res = (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body).into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        content_type
            .and_then(|c| HeaderValue::from_str(&c).ok())
            .unwrap_or_else(|| HeaderValue::from_static("application/json")),
    );
    headers.insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    res
}

/// Middleware (a route layer, so the matched route template is known).
pub async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let template = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    if !applies(req.method().as_str(), &template) {
        return next.run(req).await;
    }
    let key = match parse_key(req.headers()) {
        None => return next.run(req).await,
        Some(Err(())) => {
            return error(
                StatusCode::BAD_REQUEST,
                code::INVALID_INPUT,
                "Idempotency-Key must be 1-255 visible ASCII characters.",
            );
        }
        Some(Ok(k)) => k,
    };

    let (parts, body) = req.into_parts();
    // Unauthenticated: let the handler answer (401/403) without a row.
    let Ok(principal) = authenticate(&parts, &state, None).await else {
        return next.run(Request::from_parts(parts, body)).await;
    };
    let bytes: Bytes = match to_bytes(body, MAX_REQUEST_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                code::PAYLOAD_TOO_LARGE,
                "The request body is too large.",
            );
        }
    };
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let keyed = KeyedRequest {
        actor: actor_key(&principal),
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        key,
        hash: request_hash(&parts.method, &path_and_query, &bytes),
    };

    match claim(&state.db, &keyed, now()).await {
        Ok(Claim::Run) => {}
        Ok(Claim::Replay {
            status,
            content_type,
            body,
        }) => return replay(status, content_type, body),
        Ok(Claim::InUse) => {
            return error(
                StatusCode::CONFLICT,
                code::IDEMPOTENCY_KEY_IN_USE,
                "A request with this Idempotency-Key is still being processed.",
            );
        }
        Ok(Claim::Mismatch) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                code::IDEMPOTENCY_KEY_REUSED,
                "This Idempotency-Key was already used with a different request body.",
            );
        }
        Err(e) => {
            tracing::warn!("Idempotency store unavailable, running without it: {e}");
            return next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
        }
    }

    let res = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if !res.status().is_success() {
        let _ = release(&state.db, &keyed).await;
        return res;
    }
    let (res_parts, res_body) = res.into_parts();
    let content_type = res_parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // Mutation responses are small JSON documents; only a broken body fails.
    let body = match to_bytes(res_body, MAX_RESPONSE_BODY).await {
        Ok(b) => b,
        Err(e) => {
            let _ = release(&state.db, &keyed).await;
            tracing::warn!("Idempotent response could not be read: {e}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                code::INTERNAL,
                "The response could not be read.",
            );
        }
    };
    let stored = match std::str::from_utf8(&body) {
        Ok(text) if body.len() <= MAX_STORED_BODY => {
            complete(
                &state.db,
                &keyed,
                res_parts.status.as_u16(),
                content_type.as_deref(),
                text,
            )
            .await
        }
        _ => Err(anyhow::anyhow!("response too large or not UTF-8")),
    };
    if let Err(e) = stored {
        tracing::debug!("Idempotent response not stored: {e:#}");
        let _ = release(&state.db, &keyed).await;
    }
    Response::from_parts(res_parts, Body::from(body))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use axum::{Router, routing::post};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::RateLimiter;

    async fn state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|e| panic!("docker: {e}")),
            rate_limiter: RateLimiter::new(1000, 1000.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    /// A router whose POST /api/v1/things counts runs and answers `status`.
    fn app(state: AppState, runs: Arc<AtomicUsize>, status: StatusCode) -> Router {
        Router::new()
            .route(
                "/api/v1/things",
                post(move |body: Bytes| {
                    let runs = runs.clone();
                    async move {
                        let n = runs.fetch_add(1, Ordering::SeqCst) + 1;
                        let text = String::from_utf8_lossy(&body).to_string();
                        (
                            status,
                            axum::Json(serde_json::json!({"success": true, "data": {"run": n, "echo": text}})),
                        )
                    }
                }),
            )
            .route_layer(axum::middleware::from_fn_with_state(state.clone(), layer))
            .with_state(state)
            .layer(axum::Extension(crate::auth::LocalSocket))
    }

    fn post_req(key: Option<&str>, body: &str) -> Request {
        let mut b =
            Request::post("/api/v1/things").header(header::CONTENT_TYPE, "application/json");
        if let Some(k) = key {
            b = b.header(HEADER, k);
        }
        b.body(Body::from(body.to_string())).unwrap_or_default()
    }

    async fn send(app: &Router, req: Request) -> (StatusCode, HeaderMap, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(req)
            .await
            .unwrap_or_else(|e| match e {});
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap_or_default();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or_default(),
        )
    }

    #[test]
    fn applies_to_mutations_outside_the_exclusions() {
        assert!(applies("POST", "/api/v1/apps/{id}/deploy"));
        assert!(applies("POST", "/api/v1/apps"));
        assert!(applies("DELETE", "/api/v1/apps/{id}"));
        assert!(applies("POST", "/api/v1/tokens"));
        assert!(!applies("GET", "/api/v1/apps"));
        assert!(!applies("POST", "/api/v1/auth/login"));
        assert!(!applies("POST", "/api/v1/webhooks/github/{app_id}"));
        assert!(!applies("POST", "/api/v1/terminal/tickets"));
        assert!(!applies("POST", "/api/v1/apps/{id}/upload"));
        assert!(!applies("POST", "/webhooks/source/github/events"));
    }

    #[tokio::test]
    async fn repeats_replay_the_first_response() {
        let state = state().await;
        let runs = Arc::new(AtomicUsize::new(0));
        let app = app(state, runs.clone(), StatusCode::CREATED);

        let (s1, h1, b1) = send(&app, post_req(Some("k-1"), r#"{"a":1}"#)).await;
        let (s2, h2, b2) = send(&app, post_req(Some("k-1"), r#"{"a":1}"#)).await;
        assert_eq!((s1, s2), (StatusCode::CREATED, StatusCode::CREATED));
        assert_eq!(b1, b2, "the replay is byte-for-byte the first answer");
        assert_eq!(runs.load(Ordering::SeqCst), 1, "the handler ran once");
        assert!(h1.get(REPLAYED_HEADER).is_none());
        assert_eq!(
            h2.get(REPLAYED_HEADER).and_then(|v| v.to_str().ok()),
            Some("true")
        );

        // Without a key, or with another key, it runs again.
        send(&app, post_req(None, r#"{"a":1}"#)).await;
        send(&app, post_req(Some("k-2"), r#"{"a":1}"#)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_different_body_with_the_same_key_is_refused() {
        let state = state().await;
        let runs = Arc::new(AtomicUsize::new(0));
        let app = app(state, runs.clone(), StatusCode::OK);
        send(&app, post_req(Some("k"), r#"{"a":1}"#)).await;
        let (status, _, body) = send(&app, post_req(Some("k"), r#"{"a":2}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failures_are_not_stored_so_a_retry_runs() {
        let state = state().await;
        let runs = Arc::new(AtomicUsize::new(0));
        let app = app(state.clone(), runs.clone(), StatusCode::CONFLICT);
        send(&app, post_req(Some("k"), "{}")).await;
        send(&app, post_req(Some("k"), "{}")).await;
        assert_eq!(runs.load(Ordering::SeqCst), 2, "a 409 is not replayed");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM idempotency_keys")
            .fetch_one(&state.db)
            .await
            .unwrap_or(-1);
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn invalid_keys_are_rejected() {
        let state = state().await;
        let app = app(state, Arc::new(AtomicUsize::new(0)), StatusCode::OK);
        let long = "x".repeat(300);
        let (status, _, body) = send(&app, post_req(Some(&long), "{}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["field"], "Idempotency-Key");
    }

    #[tokio::test]
    async fn running_keys_conflict_and_old_keys_expire() {
        let state = state().await;
        let req = KeyedRequest {
            actor: "local".into(),
            method: "POST".into(),
            path: "/api/v1/apps/web/deploy".into(),
            key: "k".into(),
            hash: "h".into(),
        };
        let t = 1_000_000;
        assert_eq!(claim(&state.db, &req, t).await.ok(), Some(Claim::Run));
        assert_eq!(claim(&state.db, &req, t + 1).await.ok(), Some(Claim::InUse));
        // A running row left behind by a crash can be retried after a while.
        assert_eq!(
            claim(&state.db, &req, t + STALE_RUNNING_SECS + 1)
                .await
                .ok(),
            Some(Claim::Run)
        );
        complete(&state.db, &req, 202, Some("application/json"), "{\"ok\":1}")
            .await
            .unwrap_or_else(|e| panic!("complete: {e}"));
        let stored: Option<String> = sqlx::query_scalar("SELECT response FROM idempotency_keys")
            .fetch_one(&state.db)
            .await
            .unwrap_or_default();
        assert!(
            !stored.unwrap_or_default().contains("ok"),
            "stored responses are encrypted"
        );
        let later = t + STALE_RUNNING_SECS + 2;
        assert!(matches!(
            claim(&state.db, &req, later).await,
            Ok(Claim::Replay { status: 202, .. })
        ));
        assert_eq!(
            claim(&state.db, &req, later + TTL_SECS + 1).await.ok(),
            Some(Claim::Run),
            "after 24 h the key is new again"
        );
    }

    #[tokio::test]
    async fn prune_bounds_the_table() {
        let state = state().await;
        let t = 2_000_000;
        for i in 0..(MAX_ROWS + 10) {
            sqlx::query(
                "INSERT INTO idempotency_keys (actor, method, path, key, request_hash, created_at)
                 VALUES ('a', 'POST', '/p', ?, 'h', ?)",
            )
            .bind(format!("k{i}"))
            .bind(t + i)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        }
        sqlx::query(
            "INSERT INTO idempotency_keys (actor, method, path, key, request_hash, created_at)
             VALUES ('a', 'POST', '/p', 'old', 'h', 1)",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("insert: {e}"));
        prune(&state.db, t + MAX_ROWS + 10)
            .await
            .unwrap_or_else(|e| panic!("prune: {e}"));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM idempotency_keys")
            .fetch_one(&state.db)
            .await
            .unwrap_or(-1);
        assert!(n <= MAX_ROWS + 1, "{n} rows left");
        let old: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM idempotency_keys WHERE key = 'old'")
                .fetch_one(&state.db)
                .await
                .unwrap_or(-1);
        assert_eq!(old, 0, "expired keys go first");
    }
}
