// handlers/logs.rs — WebSocket live log streaming
//
// GET /api/apps/{id}/logs/stream
//
//   Streams deploy logs and container logs for a single app.
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
// GET /api/databases/logs/stream, GET /api/databases/{id}/logs/stream,
// GET /api/cron/{id}/runs/stream — see the handlers below.
//
// Authentication (all endpoints): the local Unix socket (CLI on the host),
// `?token=<api-key>` query param, or `__Host-tond_session` cookie (browser
// upgrades must come from the dashboard origin, see auth/csrf.rs). Close code 4001 on failure.
//
// Task lifecycle: every stream owns a `JoinSet` holding its producer tasks
// (deploy-log forwarder, container supervisor, cron poller). The socket loop
// (`pump`) returns as soon as the client closes, errors, or stops answering
// pings; dropping the JoinSet then aborts every producer, and the container
// supervisor's own JoinSet aborts its Docker followers. Nothing outlives the
// socket.
//
// Sources:
//   - Deploy logs come from `log_bus` (published by deploy::log_deploy), not
//     from polling the table.
//   - Container logs: a supervisor re-reads the relevant container ids every
//     CONTAINER_RESCAN (one small indexed query) and starts/stops Docker log
//     followers, so containers started after the socket opened (redeploys,
//     new apps, restarts) are picked up.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use tokio::sync::{broadcast, mpsc};
use tokio::task::{AbortHandle, JoinSet};

use crate::{
    auth::{self, LocalPeer},
    docker,
    handlers::auth::{extract_session_token, validate_session},
    log_bus::{self, DeployEvent},
    state::AppState,
};
use tond_common::{CronJobRun, DeployLog, ManagedDatabase};

/// How often container supervisors re-read the container set.
const CONTAINER_RESCAN: Duration = Duration::from_secs(5);
/// A follower that ended (container stopped) is restarted at most this often.
const FOLLOWER_RESTART_BACKOFF: Duration = Duration::from_secs(30);
/// Ping interval; a dead peer is detected when a ping fails to send.
const PING_INTERVAL: Duration = Duration::from_secs(30);
/// How often the cron run stream checks for finished runs.
const CRON_POLL: Duration = Duration::from_secs(2);

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

// ── Socket plumbing ───────────────────────────────────────────────────────────

pub(crate) async fn close_with(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

/// Forward frames from `rx` to the socket until the client goes away: a
/// Close frame, a receive error, the end of the stream, a failed send, or a
/// failed ping. Returns when the stream must be torn down.
async fn pump(socket: &mut WebSocket, rx: &mut mpsc::Receiver<String>) {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // the first tick completes immediately

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                None => return, // every producer ended
            },
            client_msg = socket.recv() => match client_msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {} // pings are answered by axum; ignore pongs/text
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Default::default())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Run a stream's producers until the socket closes, then abort them all.
async fn serve(socket: &mut WebSocket, mut rx: mpsc::Receiver<String>, tasks: JoinSet<()>) {
    pump(socket, &mut rx).await;
    // Dropping the JoinSet aborts every producer task (and, transitively, the
    // Docker followers owned by container supervisors).
    drop(tasks);
}

// ── Deploy log forwarding ─────────────────────────────────────────────────────

/// Which deploy log lines a stream wants and how frames are shaped.
enum DeployLogScope {
    /// One app; frames carry no app fields.
    App(String),
    /// All apps; frames carry `app_id` / `app_name`.
    All,
}

/// Forward deploy log lines from the bus. `already_sent` holds the ids of the
/// snapshot rows (the subscription starts before the snapshot query, so a
/// line may be in both).
async fn forward_deploy_logs(
    mut bus: broadcast::Receiver<Arc<DeployEvent>>,
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    scope: DeployLogScope,
    already_sent: HashSet<String>,
    mut app_names: HashMap<String, String>,
) {
    loop {
        let event = match bus.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                let frame = json!({
                    "type": "deploy",
                    "level": "warn",
                    "message": format!("{skipped} log line(s) skipped because the connection was too slow; reload to see them"),
                    "ts": chrono::Utc::now().to_rfc3339(),
                });
                if tx.send(frame.to_string()).await.is_err() {
                    return;
                }
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let DeployEvent::Log(event) = event.as_ref() else {
            continue;
        };
        if already_sent.contains(&event.id) {
            continue;
        }
        let frame = match &scope {
            DeployLogScope::App(app_id) => {
                if &event.app_id != app_id {
                    continue;
                }
                json!({
                    "type": "deploy",
                    "level": event.level,
                    "message": event.message,
                    "ts": event.created_at,
                })
            }
            DeployLogScope::All => {
                if !app_names.contains_key(&event.app_id) {
                    let name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM apps WHERE id = ?")
                            .bind(&event.app_id)
                            .fetch_optional(&db)
                            .await
                            .unwrap_or(None);
                    app_names.insert(event.app_id.clone(), name.unwrap_or_default());
                }
                json!({
                    "type": "deploy",
                    "app_id": event.app_id,
                    "app_name": app_names.get(&event.app_id).cloned().unwrap_or_default(),
                    "level": event.level,
                    "message": event.message,
                    "ts": event.created_at,
                })
            }
        };
        if tx.send(frame.to_string()).await.is_err() {
            return;
        }
    }
}

// ── Container log supervision ─────────────────────────────────────────────────

/// Which containers a stream follows.
#[derive(Clone)]
enum ContainerScope {
    App(String),
    AllApps,
    Database(String),
    AllDatabases,
}

/// A container to follow and the fields its frames carry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FollowTarget {
    /// Stable key (app or database id).
    key: String,
    container_id: String,
    /// `(id_field, name_field, id, name)` added to each frame, if tagged.
    tag: Option<(&'static str, &'static str, String, String)>,
}

impl ContainerScope {
    async fn targets(&self, db: &sqlx::SqlitePool) -> Result<Vec<FollowTarget>, sqlx::Error> {
        let rows: Vec<(String, String, String)> = match self {
            Self::App(id) => {
                sqlx::query_as(
                    "SELECT id, name, container_id FROM apps WHERE id = ? AND container_id IS NOT NULL",
                )
                .bind(id)
                .fetch_all(db)
                .await?
            }
            Self::AllApps => {
                sqlx::query_as(
                    "SELECT id, name, container_id FROM apps WHERE container_id IS NOT NULL",
                )
                .fetch_all(db)
                .await?
            }
            Self::Database(id) => {
                sqlx::query_as(
                    "SELECT id, name, container_id FROM databases WHERE id = ? AND container_id IS NOT NULL",
                )
                .bind(id)
                .fetch_all(db)
                .await?
            }
            Self::AllDatabases => {
                sqlx::query_as(
                    "SELECT id, name, container_id FROM databases WHERE container_id IS NOT NULL",
                )
                .fetch_all(db)
                .await?
            }
        };
        let tag_fields = match self {
            Self::AllApps => Some(("app_id", "app_name")),
            Self::AllDatabases => Some(("db_id", "db_name")),
            Self::App(_) | Self::Database(_) => None,
        };
        Ok(rows
            .into_iter()
            .map(|(id, name, container_id)| FollowTarget {
                tag: tag_fields.map(|(i, n)| (i, n, id.clone(), name)),
                key: id,
                container_id,
            })
            .collect())
    }
}

/// State of a running (or recently ended) follower.
struct Follower {
    container_id: String,
    started: Instant,
    handle: AbortHandle,
}

/// What to change so the followers match `desired`.
#[derive(Debug, Default, PartialEq, Eq)]
struct FollowerPlan {
    /// Keys whose follower must be aborted (gone, or container replaced).
    stop: Vec<String>,
    /// Targets that need a (new) follower.
    start: Vec<FollowTarget>,
}

/// Compare current followers with the desired targets. `current` maps key →
/// (container id, follower finished?, follower age). A finished follower of a
/// still-desired container (the container stopped and may restart with the
/// same id) is restarted once it is older than FOLLOWER_RESTART_BACKOFF.
fn plan_followers(
    current: &HashMap<String, (String, bool, Duration)>,
    desired: &[FollowTarget],
) -> FollowerPlan {
    let desired_keys: HashSet<&str> = desired.iter().map(|t| t.key.as_str()).collect();
    let mut plan = FollowerPlan::default();
    for key in current.keys() {
        if !desired_keys.contains(key.as_str()) {
            plan.stop.push(key.clone());
        }
    }
    for target in desired {
        match current.get(&target.key) {
            None => plan.start.push(target.clone()),
            Some((cid, _, _)) if cid != &target.container_id => {
                plan.stop.push(target.key.clone());
                plan.start.push(target.clone());
            }
            Some((_, true, age)) if *age >= FOLLOWER_RESTART_BACKOFF => {
                plan.start.push(target.clone());
            }
            Some(_) => {}
        }
    }
    plan.stop.sort();
    plan
}

/// Keep one Docker log follower per container in `scope`, rescanning every
/// CONTAINER_RESCAN. Followers live in a JoinSet owned by this future, so
/// aborting the supervisor aborts them too.
async fn supervise_containers(
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    docker: bollard::Docker,
    scope: ContainerScope,
) {
    let mut set: JoinSet<()> = JoinSet::new();
    let mut followers: HashMap<String, Follower> = HashMap::new();
    let mut tick = tokio::time::interval(CONTAINER_RESCAN);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut first_scan = true;
    let mut last_scan_unix = chrono::Utc::now().timestamp();

    loop {
        tick.tick().await;
        if tx.is_closed() {
            return;
        }
        let scan_unix = chrono::Utc::now().timestamp();
        let Ok(desired) = scope.targets(&db).await else {
            continue;
        };
        // Reap finished followers so the JoinSet does not grow.
        while set.try_join_next().is_some() {}

        let current: HashMap<String, (String, bool, Duration)> = followers
            .iter()
            .map(|(k, f)| {
                (
                    k.clone(),
                    (
                        f.container_id.clone(),
                        f.handle.is_finished(),
                        f.started.elapsed(),
                    ),
                )
            })
            .collect();
        let plan = plan_followers(&current, &desired);

        for key in &plan.stop {
            if let Some(f) = followers.remove(key) {
                f.handle.abort();
            }
        }
        for target in plan.start {
            if let Some(old) = followers.remove(&target.key) {
                old.handle.abort();
            }
            // The first scan follows only new output (the snapshot already
            // covered history). Containers found later — a redeploy, a new
            // app, a restart — are read from the previous scan onward so
            // their first lines are not lost.
            let since = if first_scan {
                None
            } else {
                Some(last_scan_unix)
            };
            let handle = set.spawn(follow_container(
                tx.clone(),
                docker.clone(),
                target.container_id.clone(),
                target.tag.clone(),
                since,
            ));
            followers.insert(
                target.key.clone(),
                Follower {
                    container_id: target.container_id,
                    started: Instant::now(),
                    handle,
                },
            );
        }
        first_scan = false;
        last_scan_unix = scan_unix;
    }
}

/// Follow one container's logs until the stream ends or the receiver is gone.
async fn follow_container(
    tx: mpsc::Sender<String>,
    docker: bollard::Docker,
    container_id: String,
    tag: Option<(&'static str, &'static str, String, String)>,
    since_unix: Option<i64>,
) {
    let mut options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .timestamps(true);
    options = match since_unix.and_then(|s| i32::try_from(s).ok()) {
        Some(since) => options.since(since),
        None => options.tail("0"),
    };

    let mut stream = docker.logs(&container_id, Some(options.build()));
    while let Some(result) = stream.next().await {
        match result {
            Ok(output) => {
                let raw = output.to_string();
                let (ts, line) = split_docker_ts(&raw);
                let mut frame = json!({ "type": "container", "ts": ts, "line": line });
                if let Some((id_field, name_field, id, name)) = &tag {
                    frame[*id_field] = json!(id);
                    frame[*name_field] = json!(name);
                }
                if tx.send(frame.to_string()).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!("container log stream ended ({}): {}", container_id, e);
                return;
            }
        }
    }
}

// ── App log stream ────────────────────────────────────────────────────────────

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
    if !authenticate(&state, &query, &headers, local).await {
        close_with(&mut socket, 4001, "Authentication required").await;
        return;
    }

    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(app)) = app else {
        close_with(&mut socket, 4004, "App not found").await;
        return;
    };

    // Subscribe before the snapshot query so no line falls in between.
    let bus = log_bus::subscribe();

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
    let snapshot = json!({
        "type": "snapshot",
        "deploy_logs": deploy_logs.iter().rev().map(|l| json!({
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

    // ── Producers ──
    let (tx, rx) = mpsc::channel::<String>(256);
    let mut tasks = JoinSet::new();
    tasks.spawn(forward_deploy_logs(
        bus,
        tx.clone(),
        state.db.clone(),
        DeployLogScope::App(app.id.clone()),
        deploy_logs.into_iter().map(|l| l.id).collect(),
        HashMap::new(),
    ));
    tasks.spawn(supervise_containers(
        tx,
        state.db.clone(),
        state.docker.clone(),
        ContainerScope::App(app.id),
    ));

    serve(&mut socket, rx, tasks).await;
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
    if !authenticate(&state, &query, &headers, local).await {
        close_with(&mut socket, 4001, "Authentication required").await;
        return;
    }

    let bus = log_bus::subscribe();

    // App-id → name lookup for frames.
    let app_names: HashMap<String, String> = sqlx::query_as("SELECT id, name FROM apps")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    // ── Send snapshot — most recent 100 deploy logs across all apps ──
    let deploy_logs = sqlx::query_as::<_, DeployLog>(
        "SELECT * FROM deploy_logs ORDER BY created_at DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let snapshot = json!({
        "type": "snapshot",
        "deploy_logs": deploy_logs.iter().rev().map(|l| json!({
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

    let (tx, rx) = mpsc::channel::<String>(512);
    let mut tasks = JoinSet::new();
    tasks.spawn(forward_deploy_logs(
        bus,
        tx.clone(),
        state.db.clone(),
        DeployLogScope::All,
        deploy_logs.into_iter().map(|l| l.id).collect(),
        app_names,
    ));
    tasks.spawn(supervise_containers(
        tx,
        state.db.clone(),
        state.docker.clone(),
        ContainerScope::AllApps,
    ));

    serve(&mut socket, rx, tasks).await;
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
    if !authenticate(&state, &query, &headers, local).await {
        close_with(&mut socket, 4001, "Authentication required").await;
        return;
    }

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

    let (tx, rx) = mpsc::channel::<String>(512);
    let mut tasks = JoinSet::new();
    tasks.spawn(supervise_containers(
        tx,
        state.db.clone(),
        state.docker.clone(),
        ContainerScope::AllDatabases,
    ));
    serve(&mut socket, rx, tasks).await;
}

// ── Database log stream ───────────────────────────────────────────────────────

/// Axum handler — upgrades to WebSocket and streams container logs for a
/// single managed database. No deploy logs (databases have no build
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
    if !authenticate(&state, &query, &headers, local).await {
        close_with(&mut socket, 4001, "Authentication required").await;
        return;
    }

    let db_row = sqlx::query_as::<_, ManagedDatabase>(
        "SELECT * FROM databases WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(db_row)) = db_row else {
        close_with(&mut socket, 4004, "Database not found").await;
        return;
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

    let (tx, rx) = mpsc::channel::<String>(256);
    let mut tasks = JoinSet::new();
    tasks.spawn(supervise_containers(
        tx,
        state.db.clone(),
        state.docker.clone(),
        ContainerScope::Database(db_row.id),
    ));
    serve(&mut socket, rx, tasks).await;
}

/// Returns true if the request carries valid authentication. `local` is true for
/// requests accepted on the trusted local Unix socket (the CLI on the host).
pub(crate) async fn authenticate(
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
// Authentication: ?token= query param or __Host-tond_session cookie.

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

fn run_json(r: &CronJobRun) -> serde_json::Value {
    json!({
        "id": r.id,
        "status": r.status,
        "exit_code": r.exit_code,
        "output": r.output,
        "started_at": r.started_at,
        "finished_at": r.finished_at,
    })
}

async fn handle_cron_socket(
    mut socket: WebSocket,
    state: AppState,
    id: String,
    query: LogStreamQuery,
    headers: HeaderMap,
    local: bool,
) {
    if !authenticate(&state, &query, &headers, local).await {
        close_with(&mut socket, 4001, "Authentication required").await;
        return;
    }

    let job_row = sqlx::query_as::<_, tond_common::CronJob>(
        "SELECT * FROM cron_jobs WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(&id)
    .bind(&id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(job)) = job_row else {
        close_with(&mut socket, 4004, "Cron job not found").await;
        return;
    };

    // ── Send snapshot — most recent run ──
    let latest_run: Option<CronJobRun> = sqlx::query_as(
        "SELECT * FROM cron_job_runs WHERE cron_job_id = ? ORDER BY started_at DESC LIMIT 1",
    )
    .bind(&job.id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let snapshot = json!({ "type": "snapshot", "run": latest_run.as_ref().map(run_json) });
    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let last_run_id = latest_run.map(|r| r.id).unwrap_or_default();
    let (tx, rx) = mpsc::channel::<String>(64);
    let mut tasks = JoinSet::new();
    tasks.spawn(poll_cron_runs(tx, state.db.clone(), job.id, last_run_id));
    serve(&mut socket, rx, tasks).await;
}

/// Poll `cron_job_runs` for new completed runs for a given job (an indexed
/// lookup on (cron_job_id, started_at)). Emits a `run` frame for each new row.
async fn poll_cron_runs(
    tx: mpsc::Sender<String>,
    db: sqlx::SqlitePool,
    job_id: String,
    initial_last_run_id: String,
) {
    let mut last_run_id = initial_last_run_id;
    let mut interval = tokio::time::interval(CRON_POLL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if tx.is_closed() {
            return;
        }

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
                    let msg = json!({ "type": "run", "run": run_json(run) });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_bus::DeployLogEvent;

    fn target(key: &str, cid: &str) -> FollowTarget {
        FollowTarget {
            key: key.to_string(),
            container_id: cid.to_string(),
            tag: None,
        }
    }

    #[test]
    fn plan_followers_starts_new_stops_gone_and_replaces_changed_containers() {
        let current = HashMap::from([
            (
                "same".to_string(),
                ("c1".to_string(), false, Duration::from_secs(1)),
            ),
            (
                "gone".to_string(),
                ("c2".to_string(), false, Duration::from_secs(1)),
            ),
            (
                "redeployed".to_string(),
                ("old".to_string(), false, Duration::from_secs(1)),
            ),
        ]);
        let desired = vec![
            target("same", "c1"),
            target("redeployed", "new"),
            target("fresh", "c3"),
        ];

        let plan = plan_followers(&current, &desired);
        assert_eq!(
            plan.stop,
            vec!["gone".to_string(), "redeployed".to_string()]
        );
        assert_eq!(
            plan.start,
            vec![target("redeployed", "new"), target("fresh", "c3")]
        );
    }

    #[test]
    fn plan_followers_restarts_ended_followers_only_after_backoff() {
        let recent = HashMap::from([(
            "a".to_string(),
            ("c1".to_string(), true, Duration::from_secs(5)),
        )]);
        assert_eq!(
            plan_followers(&recent, &[target("a", "c1")]),
            FollowerPlan::default(),
            "a follower that just ended is not restarted immediately"
        );

        let old = HashMap::from([(
            "a".to_string(),
            ("c1".to_string(), true, FOLLOWER_RESTART_BACKOFF),
        )]);
        assert_eq!(
            plan_followers(&old, &[target("a", "c1")]).start,
            vec![target("a", "c1")],
            "a stopped container that may have restarted is followed again"
        );
    }

    fn event(id: &str, app_id: &str) -> DeployLogEvent {
        DeployLogEvent {
            id: id.to_string(),
            app_id: app_id.to_string(),
            deployment_id: None,
            level: "info".to_string(),
            message: format!("line {id}"),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn test_db() -> sqlx::SqlitePool {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("sqlite: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        db
    }

    #[tokio::test]
    async fn deploy_log_forwarder_filters_by_app_and_skips_snapshot_rows() {
        let db = test_db().await;
        let (bus_tx, bus_rx) = broadcast::channel(16);
        let (tx, mut rx) = mpsc::channel(16);
        let task = tokio::spawn(forward_deploy_logs(
            bus_rx,
            tx,
            db,
            DeployLogScope::App("app-a".to_string()),
            HashSet::from(["in-snapshot".to_string()]),
            HashMap::new(),
        ));

        for ev in [
            event("in-snapshot", "app-a"),
            event("other-app", "app-b"),
            event("new", "app-a"),
        ] {
            let _ = bus_tx.send(Arc::new(DeployEvent::Log(ev)));
        }

        let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("forwarder should emit a frame"))
            .unwrap_or_else(|| panic!("channel closed"));
        let value: serde_json::Value =
            serde_json::from_str(&frame).unwrap_or_else(|err| panic!("json: {err}"));
        assert_eq!(value["message"], "line new");
        assert!(
            value.get("app_id").is_none(),
            "single-app frames are untagged"
        );
        assert!(
            rx.try_recv().is_err(),
            "only the new line of app-a is forwarded"
        );
        task.abort();
    }

    #[tokio::test]
    async fn dropping_the_stream_task_set_ends_every_producer() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::channel::<String>(16);
        let mut tasks = JoinSet::new();
        tasks.spawn(forward_deploy_logs(
            log_bus::subscribe(),
            tx.clone(),
            db.clone(),
            DeployLogScope::All,
            HashSet::new(),
            HashMap::new(),
        ));
        tasks.spawn(poll_cron_runs(
            tx.clone(),
            db.clone(),
            "job".to_string(),
            String::new(),
        ));
        // A container supervisor with an unreachable Docker: it keeps
        // rescanning (no containers) until aborted.
        let docker = bollard::Docker::connect_with_http(
            "http://127.0.0.1:1",
            1,
            bollard::API_DEFAULT_VERSION,
        )
        .unwrap_or_else(|err| panic!("docker client: {err}"));
        tasks.spawn(supervise_containers(
            tx.clone(),
            db,
            docker,
            ContainerScope::AllApps,
        ));
        drop(tx);

        // Producers are alive: the channel stays open.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "producers keep the stream open while the socket is connected"
        );

        // The socket closed: `serve` drops the set.
        drop(tasks);
        let closed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("producers must stop promptly after the socket closes"));
        assert!(closed.is_none(), "every producer (and its sender) is gone");
    }
}
