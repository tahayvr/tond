// handlers/logs.rs — WebSocket live log streaming
//
// GET /api/apps/{id}/logs/stream
//
//   Streams deploy logs (DB-polled) and container logs (Docker follow) for a
//   single app.
//
//   On connect — snapshot:
//     {"type":"snapshot","deploy_logs":[...],"container_logs":["..."]}
//   Incremental frames:
//     {"type":"deploy","level":"info","message":"...","ts":"..."}
//     {"type":"container","line":"..."}
//
// GET /api/system/logs/stream
//
//   Streams deploy logs and container logs across *all* apps.
//   Each incremental frame carries `app_id` and `app_name` so the client can
//   distinguish sources.
//
//   On connect — snapshot of the 100 most recent deploy log rows (all apps):
//     {"type":"snapshot","deploy_logs":[{"app_id":"...","app_name":"...","level":"...","message":"...","ts":"..."},...]}
//   Incremental frames:
//     {"type":"deploy","app_id":"...","app_name":"...","level":"...","message":"...","ts":"..."}
//     {"type":"container","app_id":"...","app_name":"...","line":"..."}
//
// Authentication (both endpoints): the local Unix socket (CLI on the host),
// `?token=<api-key>` query param, or `tond_session` cookie. Close code 4001 on failure.

use axum::{
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::IntoResponse,
};
use bollard::query_parameters::LogsOptionsBuilder;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use crate::{
    auth::{self, LocalPeer},
    docker,
    handlers::auth::{extract_session_token, validate_session},
    state::AppState,
};
use tond_common::{App, CronJobRun, DeployLog, ManagedDatabase};

/// Split a Docker log line that has been requested with `timestamps(true)`.
///
/// Docker prepends an RFC3339 nanosecond timestamp followed by a single space:
///   `2026-03-20T23:27:38.123456789Z <rest of line>`
///
/// Returns `(ts, line)`.  If the line does not start with a recognisable
/// timestamp the whole string is returned as `line` and `ts` is `None`.
fn split_docker_ts(raw: &str) -> (Option<&str>, &str) {
    // A Docker timestamp is at most 35 chars (RFC3339 nano) followed by ' '
    if let Some(sp) = raw.find(' ')
        && sp > 0
        && sp <= 35
    {
        let ts = &raw[..sp];
        // Basic sanity: must contain 'T' and end with 'Z' or '+'.
        if ts.contains('T') && (ts.ends_with('Z') || ts.contains('+')) {
            return (Some(ts), &raw[sp + 1..]);
        }
    }
    (None, raw)
}

#[derive(Deserialize)]
pub struct LogStreamQuery {
    /// Optional API key passed as query parameter (for CLI / WebSocket clients
    /// that cannot set the Authorization header).
    pub token: Option<String>,
}

/// Axum handler — upgrades to WebSocket and streams logs.
pub async fn stream_logs(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogStreamQuery>,
    headers: HeaderMap,
    LocalPeer(local): LocalPeer,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, id, query, headers, local))
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    id: String,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    // ── Authenticate ──
    let authed = authenticate(&state, &query, &headers, local).await;
    if !authed {
        // Close with application error code
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    }

    // ── Resolve app ──
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let app = match app {
        Ok(Some(a)) => a,
        _ => {
            let _ = socket
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4004,
                    reason: "App not found".into(),
                })))
                .await;
            return;
        }
    };

    // ── Send snapshot ──
    let deploy_logs = sqlx::query_as::<_, DeployLog>(
        "SELECT * FROM deploy_logs WHERE app_id = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let container_logs = if let Some(ref cid) = app.container_id {
        docker::get_logs(&state.docker, cid, 100)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    // Deploy logs arrive newest-first from DB; reverse so they appear chronologically
    let mut ordered_deploy: Vec<&DeployLog> = deploy_logs.iter().collect();
    ordered_deploy.reverse();

    let snapshot = json!({
        "type": "snapshot",
        "deploy_logs": ordered_deploy.iter().map(|l| json!({
            "level": l.level,
            "message": l.message,
            "ts": l.created_at,
        })).collect::<Vec<_>>(),
        "container_logs": container_logs.iter().map(|raw| {
            let (ts, line) = split_docker_ts(raw);
            json!({ "ts": ts, "line": line })
        }).collect::<Vec<_>>(),
    });

    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Track the most recent deploy log timestamp we've seen (for polling)
    let last_ts = deploy_logs
        .first()
        .map(|l| l.created_at.clone())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

    // ── Spawn streamer tasks, fan-in via mpsc ──
    let (tx, mut rx) = mpsc::channel::<String>(256);

    // Task 1: deploy log poller
    {
        let tx = tx.clone();
        let db = state.db.clone();
        let app_id = app.id.clone();
        tokio::spawn(async move {
            poll_deploy_logs(tx, db, app_id, last_ts).await;
        });
    }

    // Task 2: Docker container log follower
    if let Some(container_id) = app.container_id.clone() {
        let tx = tx.clone();
        let docker = state.docker.clone();
        tokio::spawn(async move {
            follow_container_logs(tx, docker, container_id).await;
        });
    }

    drop(tx); // drop original; tasks hold their own clones

    // ── Forward messages to WebSocket ──
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break, // all senders dropped
                }
            }
            // Handle incoming messages (pings / client close)
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // ignore pings/pongs/text from client
                }
            }
        }
    }
}

/// Poll the `deploy_logs` table once per second and emit new entries.
async fn poll_deploy_logs(
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    app_id: String,
    initial_last_ts: String,
) {
    let mut last_ts = initial_last_ts;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        let rows = sqlx::query_as::<_, DeployLog>(
            "SELECT * FROM deploy_logs WHERE app_id = ? AND created_at > ? ORDER BY created_at ASC",
        )
        .bind(&app_id)
        .bind(&last_ts)
        .fetch_all(&db)
        .await;

        match rows {
            Ok(logs) => {
                for log in &logs {
                    let msg = json!({
                        "type": "deploy",
                        "level": log.level,
                        "message": log.message,
                        "ts": log.created_at,
                    });
                    if tx.send(msg.to_string()).await.is_err() {
                        return;
                    }
                    last_ts = log.created_at.clone();
                }
            }
            Err(e) => {
                tracing::warn!("deploy log poll error: {}", e);
            }
        }
    }
}

/// Follow Docker container logs live and emit each line.
async fn follow_container_logs(
    tx: mpsc::Sender<String>,
    docker: bollard::Docker,
    container_id: String,
) {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .timestamps(true)
        .tail("0") // only new lines, not history (snapshot already sent)
        .build();

    let mut stream = docker.logs(&container_id, Some(options));

    while let Some(result) = stream.next().await {
        match result {
            Ok(output) => {
                let raw = output.to_string();
                let (ts, line) = split_docker_ts(&raw);
                let msg = json!({ "type": "container", "ts": ts, "line": line });
                if tx.send(msg.to_string()).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!("container log stream ended: {}", e);
                break;
            }
        }
    }
}

// ── System-wide log stream ────────────────────────────────────────────────────

/// Axum handler — upgrades to WebSocket and streams logs from *all* apps.
pub async fn stream_system_logs(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<LogStreamQuery>,
    headers: HeaderMap,
    LocalPeer(local): LocalPeer,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_system_socket(socket, state, query, headers, local))
}

async fn handle_system_socket(
    mut socket: WebSocket,
    state: AppState,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    // ── Authenticate ──
    let authed = authenticate(&state, &query, &headers, local).await;
    if !authed {
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    }

    // ── Load all apps ──
    let apps: Vec<App> = sqlx::query_as("SELECT * FROM apps")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    // ── Send snapshot — most recent 100 deploy logs across all apps ──
    let deploy_logs = sqlx::query_as::<_, DeployLog>(
        "SELECT dl.* FROM deploy_logs dl \
         ORDER BY dl.created_at DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Build a quick app-id → name lookup
    let app_names: std::collections::HashMap<String, String> = apps
        .iter()
        .map(|a| (a.id.clone(), a.name.clone()))
        .collect();

    // Reverse so snapshot is chronological
    let mut ordered: Vec<&DeployLog> = deploy_logs.iter().collect();
    ordered.reverse();

    let snapshot = json!({
        "type": "snapshot",
        "deploy_logs": ordered.iter().map(|l| json!({
            "app_id": l.app_id,
            "app_name": app_names.get(&l.app_id).cloned().unwrap_or_default(),
            "level": l.level,
            "message": l.message,
            "ts": l.created_at,
        })).collect::<Vec<_>>(),
    });

    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Cursor: newest deploy log we already sent
    let last_ts = deploy_logs
        .first()
        .map(|l| l.created_at.clone())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

    // ── Fan-in channel ──
    let (tx, mut rx) = mpsc::channel::<String>(512);

    // Task: deploy log poller for all apps (no app_id filter)
    {
        let tx = tx.clone();
        let db = state.db.clone();
        tokio::spawn(async move {
            poll_all_deploy_logs(tx, db, last_ts).await;
        });
    }

    // Tasks: one Docker follower per currently-running container
    for app in &apps {
        if let Some(ref cid) = app.container_id {
            let tx = tx.clone();
            let docker = state.docker.clone();
            let container_id = cid.clone();
            let app_id = app.id.clone();
            let app_name = app.name.clone();
            tokio::spawn(async move {
                follow_container_logs_tagged(tx, docker, container_id, app_id, app_name).await;
            });
        }
    }

    drop(tx);

    // ── Forward messages to WebSocket ──
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// A single deploy log row joined with the app name, used by the system stream poller.
#[derive(sqlx::FromRow)]
struct DeployLogRow {
    app_id: String,
    name: String,
    level: String,
    message: String,
    created_at: String,
}

/// Poll deploy logs for *all* apps (no app_id filter), emitting app context.
async fn poll_all_deploy_logs(
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    initial_last_ts: String,
) {
    let mut last_ts = initial_last_ts;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        // JOIN to get app name alongside each log row
        let rows = sqlx::query_as::<_, DeployLogRow>(
            "SELECT dl.app_id, a.name, dl.level, dl.message, dl.created_at \
             FROM deploy_logs dl \
             JOIN apps a ON a.id = dl.app_id \
             WHERE dl.created_at > ? \
             ORDER BY dl.created_at ASC",
        )
        .bind(&last_ts)
        .fetch_all(&db)
        .await;

        match rows {
            Ok(logs) => {
                for row in &logs {
                    let msg = json!({
                        "type": "deploy",
                        "app_id": row.app_id,
                        "app_name": row.name,
                        "level": row.level,
                        "message": row.message,
                        "ts": row.created_at,
                    });
                    if tx.send(msg.to_string()).await.is_err() {
                        return;
                    }
                    last_ts = row.created_at.clone();
                }
            }
            Err(e) => {
                tracing::warn!("system deploy log poll error: {}", e);
            }
        }
    }
}

/// Follow Docker container logs and tag each line with `app_id` / `app_name`.
async fn follow_container_logs_tagged(
    tx: mpsc::Sender<String>,
    docker: bollard::Docker,
    container_id: String,
    app_id: String,
    app_name: String,
) {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .timestamps(true)
        .tail("0")
        .build();

    let mut stream = docker.logs(&container_id, Some(options));

    while let Some(result) = stream.next().await {
        match result {
            Ok(output) => {
                let raw = output.to_string();
                let (ts, line) = split_docker_ts(&raw);
                let msg = json!({
                    "type": "container",
                    "app_id": app_id,
                    "app_name": app_name,
                    "ts": ts,
                    "line": line,
                });
                if tx.send(msg.to_string()).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!("container log stream ended ({}): {}", app_name, e);
                break;
            }
        }
    }
}

// ── All-databases log stream ──────────────────────────────────────────────────

/// Axum handler — upgrades to WebSocket and streams container logs from *all*
/// running managed databases. Each frame carries `db_id` and `db_name` so the
/// client can distinguish sources.
///
/// On connect — snapshot of the last 50 container log lines per database:
///   {"type":"snapshot","container_logs":[{"db_id":"...","db_name":"...","ts":"...","line":"..."},...]}
/// Incremental frames:
///   {"type":"container","db_id":"...","db_name":"...","ts":"...","line":"..."}
pub async fn stream_all_database_logs(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<LogStreamQuery>,
    headers: HeaderMap,
    LocalPeer(local): LocalPeer,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_all_databases_socket(socket, state, query, headers, local))
}

async fn handle_all_databases_socket(
    mut socket: WebSocket,
    state: AppState,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    // ── Authenticate ──
    let authed = authenticate(&state, &query, &headers, local).await;
    if !authed {
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    }

    // ── Load all databases ──
    let dbs: Vec<ManagedDatabase> = sqlx::query_as("SELECT * FROM databases")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    // ── Send snapshot — last 50 container log lines per running database ──
    let mut snapshot_lines: Vec<serde_json::Value> = vec![];
    for db in &dbs {
        if let Some(ref cid) = db.container_id {
            let lines = docker::get_logs(&state.docker, cid, 50)
                .await
                .unwrap_or_default();
            for raw in &lines {
                let (ts, line) = split_docker_ts(raw);
                snapshot_lines.push(json!({
                    "db_id": db.id,
                    "db_name": db.name,
                    "ts": ts,
                    "line": line,
                }));
            }
        }
    }

    let snapshot = json!({ "type": "snapshot", "container_logs": snapshot_lines });
    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // ── Fan-in channel ──
    let (tx, mut rx) = mpsc::channel::<String>(512);

    // One Docker follower per currently-running container
    for db in &dbs {
        if let Some(ref cid) = db.container_id {
            let tx = tx.clone();
            let docker = state.docker.clone();
            let container_id = cid.clone();
            let db_id = db.id.clone();
            let db_name = db.name.clone();
            tokio::spawn(async move {
                follow_container_logs_db_tagged(tx, docker, container_id, db_id, db_name).await;
            });
        }
    }

    drop(tx);

    // ── Forward messages to WebSocket ──
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Follow Docker container logs and tag each line with `db_id` / `db_name`.
async fn follow_container_logs_db_tagged(
    tx: mpsc::Sender<String>,
    docker: bollard::Docker,
    container_id: String,
    db_id: String,
    db_name: String,
) {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .timestamps(true)
        .tail("0")
        .build();

    let mut stream = docker.logs(&container_id, Some(options));

    while let Some(result) = stream.next().await {
        match result {
            Ok(output) => {
                let raw = output.to_string();
                let (ts, line) = split_docker_ts(&raw);
                let msg = json!({
                    "type": "container",
                    "db_id": db_id,
                    "db_name": db_name,
                    "ts": ts,
                    "line": line,
                });
                if tx.send(msg.to_string()).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!("db container log stream ended ({}): {}", db_name, e);
                break;
            }
        }
    }
}

// ── Database log stream ───────────────────────────────────────────────────────

/// Axum handler — upgrades to WebSocket and streams container logs for a
/// single managed database. No deploy-log polling (databases have no build
/// pipeline); only a container-log snapshot + live follow.
pub async fn stream_database_logs(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogStreamQuery>,
    headers: HeaderMap,
    LocalPeer(local): LocalPeer,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_database_socket(socket, state, id, query, headers, local))
}

async fn handle_database_socket(
    mut socket: WebSocket,
    state: AppState,
    id: String,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    // ── Authenticate ──
    let authed = authenticate(&state, &query, &headers, local).await;
    if !authed {
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    }

    // ── Resolve database ──
    let db_row = sqlx::query_as::<_, ManagedDatabase>(
        "SELECT * FROM databases WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let db_row = match db_row {
        Ok(Some(d)) => d,
        _ => {
            let _ = socket
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4004,
                    reason: "Database not found".into(),
                })))
                .await;
            return;
        }
    };

    // ── Send snapshot — last 100 container log lines ──
    let container_logs = if let Some(ref cid) = db_row.container_id {
        docker::get_logs(&state.docker, cid, 100)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    let snapshot = json!({
        "type": "snapshot",
        "container_logs": container_logs.iter().map(|raw| {
            let (ts, line) = split_docker_ts(raw);
            json!({ "ts": ts, "line": line })
        }).collect::<Vec<_>>(),
    });

    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // ── Spawn container log follower ──
    let (tx, mut rx) = mpsc::channel::<String>(256);

    if let Some(container_id) = db_row.container_id.clone() {
        let tx = tx.clone();
        let docker = state.docker.clone();
        tokio::spawn(async move {
            follow_container_logs(tx, docker, container_id).await;
        });
    }

    drop(tx);

    // ── Forward messages to WebSocket ──
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Returns true if the request carries valid authentication. `local` is true for
/// requests accepted on the trusted local Unix socket (the CLI on the host).
async fn authenticate(
    state: &AppState,
    query: &LogStreamQuery,
    headers: &HeaderMap,
    local: bool,
) -> bool {
    if local {
        return true;
    }

    // Try Bearer token from query param (?token=...)
    if let Some(ref raw_key) = query.token {
        return matches!(auth::validate_key(&state.db, raw_key).await, Ok(Some(_)));
    }

    // Try session cookie
    if let Some(token) = extract_session_token(headers) {
        return matches!(validate_session(&state.db, &token).await, Ok(Some(_)));
    }

    false
}

// ── Cron run output stream ────────────────────────────────────────────────────
//
// GET /api/cron/{id}/runs/stream
//
// Streams cron job run history for the given job as new runs complete.
//
// On connect — snapshot of the most recent completed run:
//   {"type":"snapshot","run":{...} | null}
// Incremental frames — emitted each time a new run row appears:
//   {"type":"run","run":{...}}
//
// Authentication: ?token= query param or tond_session cookie.

/// Axum handler — upgrades to WebSocket and streams cron run output.
pub async fn stream_cron_runs(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogStreamQuery>,
    headers: HeaderMap,
    LocalPeer(local): LocalPeer,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_cron_socket(socket, state, id, query, headers, local))
}

async fn handle_cron_socket(
    mut socket: WebSocket,
    state: AppState,
    id: String,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    // ── Authenticate ──
    let authed = authenticate(&state, &query, &headers, local).await;
    if !authed {
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    }

    // ── Resolve cron job ──
    let job_row = sqlx::query_as::<_, tond_common::CronJob>(
        "SELECT * FROM cron_jobs WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;

    let job = match job_row {
        Ok(Some(j)) => j,
        _ => {
            let _ = socket
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4004,
                    reason: "Cron job not found".into(),
                })))
                .await;
            return;
        }
    };

    // ── Send snapshot — most recent run ──
    let latest_run: Option<CronJobRun> = sqlx::query_as(
        "SELECT * FROM cron_job_runs WHERE cron_job_id = ? ORDER BY started_at DESC LIMIT 1",
    )
    .bind(&job.id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let snapshot = json!({
        "type": "snapshot",
        "run": latest_run.as_ref().map(|r| json!({
            "id": r.id,
            "status": r.status,
            "exit_code": r.exit_code,
            "output": r.output,
            "started_at": r.started_at,
            "finished_at": r.finished_at,
        }))
    });

    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Track the most recent run we've sent so we only forward new ones
    let last_run_id = latest_run
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_default();

    // ── Poll for new completed runs ──
    let (tx, mut rx) = mpsc::channel::<String>(64);

    {
        let tx = tx.clone();
        let db = state.db.clone();
        let job_id = job.id.clone();
        tokio::spawn(async move {
            poll_cron_runs(tx, db, job_id, last_run_id).await;
        });
    }

    drop(tx);

    // ── Forward messages to WebSocket ──
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Poll `cron_job_runs` for new completed runs for a given job.
/// Emits a `run` frame for each new row.
async fn poll_cron_runs(
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    job_id: String,
    initial_last_run_id: String,
) {
    let mut last_run_id = initial_last_run_id;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        // Fetch runs newer than the last one we sent (by started_at ordering)
        let rows: Result<Vec<CronJobRun>, _> = sqlx::query_as(
            "SELECT * FROM cron_job_runs \
             WHERE cron_job_id = ? AND id != ? AND started_at >= \
               COALESCE((SELECT started_at FROM cron_job_runs WHERE id = ?), '1970-01-01') \
             ORDER BY started_at ASC",
        )
        .bind(&job_id)
        .bind(&last_run_id)
        .bind(&last_run_id)
        .fetch_all(&db)
        .await;

        match rows {
            Ok(runs) => {
                for run in &runs {
                    // Only emit once the run is no longer "running"
                    if run.status == "running" {
                        continue;
                    }
                    let msg = json!({
                        "type": "run",
                        "run": {
                            "id": run.id,
                            "status": run.status,
                            "exit_code": run.exit_code,
                            "output": run.output,
                            "started_at": run.started_at,
                            "finished_at": run.finished_at,
                        }
                    });
                    if tx.send(msg.to_string()).await.is_err() {
                        return;
                    }
                    last_run_id = run.id.clone();
                }
            }
            Err(e) => {
                tracing::warn!("cron run poll error: {}", e);
            }
        }
    }
}
