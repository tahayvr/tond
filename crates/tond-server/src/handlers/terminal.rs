// handlers/terminal.rs — browser terminal: host shell and container exec
//
// Two kinds of sessions, both over a WebSocket (`/api/terminal/ws`):
//
//   * Container exec (`docker exec` into a running app or database) — for app
//     debugging. Needs the `terminal:host` scope (admins).
//   * Host shell (`su -l $TOND_TERMINAL_USER`) — off by default. It can only be
//     switched on from the host itself (`tond settings host-terminal on`, which
//     calls PUT /api/settings/host-terminal over the root-only Unix socket),
//     so no dashboard session or API token can enable it. When on, it needs the
//     `terminal:host` scope (admins) and every session needs step-up
//     re-authentication: the password, plus a TOTP code when 2FA is enabled.
//
// Opening a session is two steps. The dashboard first POSTs to
// /api/terminal/tickets with its session cookie (and the step-up credentials
// for the host shell); the server returns a single-use ticket valid for 30
// seconds, bound to the user, the target and the requesting dashboard origin.
// The WebSocket then presents `?ticket=`; the ticket is consumed atomically and
// the socket's Origin must match the one it was issued to. API tokens can't
// obtain tickets (they can't do step-up). Sessions close after 15 minutes without input, and opening
// and closing are written to the audit log.

use std::{
    env,
    io::{Read, Write},
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, spawn_blocking},
    time::Instant,
};
use tond_common::{ApiResponse, Scope, TondError, User, generate_raw_key, hash_key};

use crate::{
    audit_log::{AuditActor, AuditEntry, record, write_audit_log},
    auth::{
        Authz, ClientIp, LocalPeer, Principal, RateLimit,
        csrf::{origin_allowed, request_origin},
        scope,
    },
    error::AppError,
    handlers::auth::{Rejection, extract_session_token, validate_session, verify_step_up},
    state::AppState,
};

/// Settings key holding "on" when the host terminal is enabled.
const HOST_TERMINAL_SETTING: &str = "host_terminal";
/// Command that enables the host terminal (shown in the dashboard and errors).
pub const HOST_TERMINAL_ENABLE_COMMAND: &str = "sudo tond settings host-terminal on";
/// How long a terminal ticket stays valid.
const TICKET_TTL_SECS: i64 = 30;
/// Sessions without input for this long are closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalTargetKind {
    Server,
    App,
    Database,
}

impl TerminalTargetKind {
    fn as_str(&self) -> &'static str {
        match self {
            TerminalTargetKind::Server => "server",
            TerminalTargetKind::App => "app",
            TerminalTargetKind::Database => "database",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "server" => Some(TerminalTargetKind::Server),
            "app" => Some(TerminalTargetKind::App),
            "database" => Some(TerminalTargetKind::Database),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalTargetResponse {
    pub id: String,
    pub kind: TerminalTargetKind,
    pub label: String,
    pub shell_user: Option<String>,
    /// False for the host shell while it is switched off.
    pub enabled: bool,
    /// Whether opening a session needs the password (and 2FA code).
    pub requires_step_up: bool,
}

/// Body of POST /api/terminal/tickets.
#[derive(Debug, Deserialize)]
pub struct TerminalTicketRequest {
    pub target_kind: TerminalTargetKind,
    pub target_id: String,
    /// Step-up password (host shell only).
    #[serde(default)]
    pub password: Option<String>,
    /// Step-up TOTP or backup code (host shell, when 2FA is enabled).
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TerminalTicketResponse {
    pub ticket: String,
    pub expires_in: i64,
}

#[derive(Debug, Deserialize)]
pub struct TerminalWsQuery {
    pub ticket: String,
}

/// Body of PUT /api/settings/host-terminal, and the GET response.
#[derive(Debug, Serialize, Deserialize)]
pub struct HostTerminalSetting {
    pub enabled: bool,
}

/// A consumed ticket: who may open which terminal.
struct TerminalGrant {
    user: User,
    kind: TerminalTargetKind,
    target_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TerminalClientFrame {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    Close,
}

#[derive(Debug, sqlx::FromRow)]
struct TerminalWorkloadRow {
    id: String,
    name: String,
    container_id: Option<String>,
    status: String,
}

enum TerminalSessionSpec {
    Server { shell_user: String },
    Container { container_id: String },
}

enum TerminalSessionEvent {
    Output(String),
    Exit(Option<i32>),
    Error(String),
}

struct TerminalSessionRuntime {
    child: Arc<Mutex<Box<dyn Child + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_task: JoinHandle<()>,
    wait_task: JoinHandle<()>,
}

pub async fn list_targets(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let shell_user = terminal_shell_user_from_env()?;
    let host_enabled = host_terminal_enabled(&state.db).await?;
    let targets = collect_terminal_targets(&state.db, &shell_user, host_enabled).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(targets))))
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::err(message.into()))).into_response()
}

fn reject(status: StatusCode, message: impl Into<String>) -> Rejection {
    Box::new(error_response(status, message))
}

fn host_terminal_disabled_message() -> String {
    format!(
        "The host terminal is turned off. To turn it on, run `{HOST_TERMINAL_ENABLE_COMMAND}` on the server."
    )
}

/// Whether the host terminal has been switched on from the host.
pub async fn host_terminal_enabled(db: &sqlx::SqlitePool) -> Result<bool, sqlx::Error> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(HOST_TERMINAL_SETTING)
        .fetch_optional(db)
        .await?;
    Ok(value.as_deref() == Some("on"))
}

// ── GET /api/settings/host-terminal ──

pub async fn get_host_terminal(
    _rate: RateLimit,
    _auth: Principal,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let enabled = host_terminal_enabled(&state.db).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(HostTerminalSetting { enabled })),
    ))
}

// ── PUT /api/settings/host-terminal ──
// Only accepted on the root-only Unix socket (`tond settings host-terminal`).

pub async fn set_host_terminal(
    LocalPeer(local): LocalPeer,
    State(state): State<AppState>,
    Json(req): Json<HostTerminalSetting>,
) -> Response {
    if !local {
        return error_response(
            StatusCode::FORBIDDEN,
            format!(
                "The host terminal can only be switched on or off on the server itself: run `sudo tond settings host-terminal {}`.",
                if req.enabled { "on" } else { "off" }
            ),
        );
    }

    let value = if req.enabled { "on" } else { "off" };
    if let Err(e) = sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(HOST_TERMINAL_SETTING)
    .bind(value)
    .execute(&state.db)
    .await
    {
        return AppError::from(e).into_response();
    }

    record(
        &state.db,
        &AuditActor::local_socket(),
        AuditEntry::new(if req.enabled {
            "host_terminal_enabled"
        } else {
            "host_terminal_disabled"
        })
        .kind("settings"),
    )
    .await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(HostTerminalSetting {
            enabled: req.enabled,
        })),
    )
        .into_response()
}

// ── POST /api/terminal/tickets ──

pub async fn create_ticket(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TerminalTicketRequest>,
) -> Response {
    match issue_ticket(&state, ip, &headers, req).await {
        Ok(ticket) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(TerminalTicketResponse {
                ticket,
                expires_in: TICKET_TTL_SECS,
            })),
        )
            .into_response(),
        Err(response) => *response,
    }
}

async fn issue_ticket(
    state: &AppState,
    ip: Option<IpAddr>,
    headers: &HeaderMap,
    req: TerminalTicketRequest,
) -> Result<String, Rejection> {
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "Terminal sessions can't be opened with an API token. Sign in to the dashboard.",
        ));
    }
    let token = extract_session_token(headers)
        .ok_or_else(|| reject(StatusCode::UNAUTHORIZED, "Not authenticated"))?;
    let user = match validate_session(state, &token).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return Err(reject(
                StatusCode::UNAUTHORIZED,
                "Session expired or invalid",
            ));
        }
        Err(e) => return Err(Box::new(AppError::from(e).into_response())),
    };
    if !user.role().scopes().contains(Scope::TerminalHost) {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "Your role doesn't allow terminal sessions: they need the terminal:host permission. Ask an admin of this server.",
        ));
    }

    let dashboard_domain = state.settings.read().await.dashboard_domain.clone();
    let origin = request_origin(headers)
        .filter(|origin| origin_allowed(origin, &dashboard_domain))
        .map(str::to_string)
        .ok_or_else(|| {
            reject(
                StatusCode::FORBIDDEN,
                "Terminal sessions can only be opened from the dashboard",
            )
        })?;

    let shell_user = terminal_shell_user_from_env().map_err(|e| Box::new(e.into_response()))?;
    resolve_terminal_session(&state.db, &req.target_kind, &req.target_id, &shell_user)
        .await
        .map_err(|e| Box::new(AppError(e).into_response()))?;

    if req.target_kind == TerminalTargetKind::Server {
        match host_terminal_enabled(&state.db).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(reject(
                    StatusCode::FORBIDDEN,
                    host_terminal_disabled_message(),
                ));
            }
            Err(e) => return Err(Box::new(AppError::from(e).into_response())),
        }
        let password = req.password.as_deref().unwrap_or("");
        if password.is_empty() {
            return Err(reject(
                StatusCode::UNAUTHORIZED,
                "Confirm your password to open the host terminal",
            ));
        }
        verify_step_up(state, ip, &user, password, req.code.as_deref()).await?;
    }

    let ticket = generate_raw_key();
    let now = Utc::now();
    let expires = now + chrono::Duration::seconds(TICKET_TTL_SECS);
    // Opportunistic cleanup of expired tickets.
    let _ = sqlx::query("DELETE FROM terminal_tickets WHERE expires_at <= ?")
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await;
    sqlx::query(
        "INSERT INTO terminal_tickets (ticket_hash, user_id, target_kind, target_id, origin, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(hash_key(&ticket))
    .bind(&user.id)
    .bind(req.target_kind.as_str())
    .bind(&req.target_id)
    .bind(&origin)
    .bind(expires.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await
    .map_err(|e| Box::new(AppError::from(e).into_response()))?;

    Ok(ticket)
}

/// Consume a ticket presented by a WebSocket from `origin` (single use).
async fn redeem_ticket(
    state: &AppState,
    ticket: &str,
    origin: Option<&str>,
) -> Result<TerminalGrant, Rejection> {
    let origin = origin.ok_or_else(|| {
        reject(
            StatusCode::FORBIDDEN,
            "Terminal sessions can only be opened from the dashboard",
        )
    })?;

    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "DELETE FROM terminal_tickets WHERE ticket_hash = ? AND expires_at > ?
         RETURNING user_id, target_kind, target_id, origin",
    )
    .bind(hash_key(ticket))
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(&state.db)
    .await
    .map_err(|e| Box::new(AppError::from(e).into_response()))?;

    let (user_id, kind, target_id, ticket_origin) = row.ok_or_else(|| {
        reject(
            StatusCode::UNAUTHORIZED,
            "Terminal ticket is invalid, expired or already used",
        )
    })?;
    if !ticket_origin.eq_ignore_ascii_case(origin) {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "Terminal ticket was issued to a different origin",
        ));
    }
    let kind = TerminalTargetKind::parse(&kind)
        .ok_or_else(|| reject(StatusCode::BAD_REQUEST, "Unknown terminal target"))?;

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| Box::new(AppError::from(e).into_response()))?
        .filter(|u: &User| {
            u.disabled_at.is_none() && u.role().scopes().contains(Scope::TerminalHost)
        })
        .ok_or_else(|| {
            reject(
                StatusCode::FORBIDDEN,
                "Your role doesn't allow terminal sessions: they need the terminal:host permission.",
            )
        })?;

    // The host terminal may have been switched off since the ticket was issued.
    if kind == TerminalTargetKind::Server {
        match host_terminal_enabled(&state.db).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(reject(
                    StatusCode::FORBIDDEN,
                    host_terminal_disabled_message(),
                ));
            }
            Err(e) => return Err(Box::new(AppError::from(e).into_response())),
        }
    }

    Ok(TerminalGrant {
        user,
        kind,
        target_id,
    })
}

// ── GET /api/terminal/ws?ticket=… ──

pub async fn terminal_ws(
    ws: WebSocketUpgrade,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    Query(query): Query<TerminalWsQuery>,
    headers: HeaderMap,
) -> Response {
    match redeem_ticket(&state, &query.ticket, request_origin(&headers)).await {
        Ok(grant) => ws
            .on_upgrade(move |socket| handle_terminal_socket(socket, state, grant, ip))
            .into_response(),
        Err(response) => *response,
    }
}

/// Human-readable target description for the audit log.
async fn target_label(db: &sqlx::SqlitePool, kind: &TerminalTargetKind, id: &str) -> String {
    let sql = match kind {
        TerminalTargetKind::Server => return "host shell".to_string(),
        TerminalTargetKind::App => "SELECT name FROM apps WHERE id = ?",
        TerminalTargetKind::Database => "SELECT name FROM databases WHERE id = ?",
    };
    let name: Option<String> = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    format!(
        "{} {}",
        kind.as_str(),
        name.unwrap_or_else(|| id.to_string())
    )
}

async fn handle_terminal_socket(
    mut socket: WebSocket,
    state: AppState,
    grant: TerminalGrant,
    ip: Option<IpAddr>,
) {
    let shell_user = match terminal_shell_user_from_env() {
        Ok(user) => user,
        Err(err) => {
            let _ = send_terminal_error(&mut socket, &err.0.to_string()).await;
            close_socket(&mut socket, 4400, "Terminal is not configured").await;
            return;
        }
    };

    let spec = match resolve_terminal_session(&state.db, &grant.kind, &grant.target_id, &shell_user)
        .await
    {
        Ok(spec) => spec,
        Err(err) => {
            let _ = send_terminal_error(&mut socket, &err.to_string()).await;
            close_socket(&mut socket, 4404, "Terminal target not found").await;
            return;
        }
    };

    let runtime = match spawn_terminal_process(&spec) {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ =
                send_terminal_error(&mut socket, &format!("Failed to start terminal: {err}")).await;
            close_socket(&mut socket, 4500, "Failed to start terminal").await;
            return;
        }
    };

    let label = target_label(&state.db, &grant.kind, &grant.target_id).await;
    let (app_id, app_name) = match grant.kind {
        TerminalTargetKind::App => (Some(grant.target_id.as_str()), None::<&str>),
        _ => (None, None),
    };
    let actor = AuditActor::user(&grant.user.id, &grant.user.email, ip.map(|i| i.to_string()));
    write_audit_log(
        &state.db,
        app_id,
        app_name,
        "terminal_opened",
        &actor,
        Some(&label),
    )
    .await;
    let started = Instant::now();

    let close_reason = run_terminal_session(&mut socket, runtime).await;

    write_audit_log(
        &state.db,
        app_id,
        app_name,
        "terminal_closed",
        &actor,
        Some(&format!(
            "{label}; {close_reason} after {}s",
            started.elapsed().as_secs()
        )),
    )
    .await;
}

/// Pump data between the socket and the PTY until either side ends or the
/// session is idle for IDLE_TIMEOUT. Returns why the session ended.
async fn run_terminal_session(
    socket: &mut WebSocket,
    runtime: TerminalSessionRuntime,
) -> &'static str {
    if socket
        .send(Message::Text(json!({ "type": "ready" }).to_string().into()))
        .await
        .is_err()
    {
        shutdown_terminal_session(runtime).await;
        return "client disconnected";
    }

    let (tx, mut rx) = mpsc::channel::<TerminalSessionEvent>(256);
    let reader_task = spawn_terminal_reader(runtime.master.clone(), tx.clone());
    let wait_task = spawn_terminal_waiter(runtime.child.clone(), tx);
    let runtime = TerminalSessionRuntime {
        reader_task,
        wait_task,
        ..runtime
    };

    let idle = tokio::time::sleep(IDLE_TIMEOUT);
    tokio::pin!(idle);

    let reason = loop {
        tokio::select! {
            output = rx.recv() => {
                match output {
                    Some(TerminalSessionEvent::Output(text)) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break "client disconnected";
                        }
                    }
                    Some(TerminalSessionEvent::Exit(code)) => {
                        let _ = socket
                            .send(Message::Text(json!({
                                "type": "exit",
                                "code": code
                            }).to_string().into()))
                            .await;
                        break "shell exited";
                    }
                    Some(TerminalSessionEvent::Error(message)) => {
                        let _ = send_terminal_error(socket, &message).await;
                        break "terminal error";
                    }
                    None => break "terminal ended",
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Text(text))) => {
                        idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                        if !handle_terminal_client_message(&runtime, text.as_str()).await {
                            break "closed by client";
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break "client disconnected",
                    Some(Err(_)) => break "client disconnected",
                    _ => {}
                }
            }
            () = &mut idle => {
                let _ = send_terminal_error(
                    socket,
                    "Session closed after 15 minutes without input.",
                ).await;
                close_socket(socket, 4408, "Idle timeout").await;
                break "idle timeout";
            }
        }
    };

    shutdown_terminal_session(runtime).await;
    reason
}

async fn handle_terminal_client_message(runtime: &TerminalSessionRuntime, raw: &str) -> bool {
    let frame = match serde_json::from_str::<TerminalClientFrame>(raw) {
        Ok(frame) => frame,
        Err(_) => return false,
    };

    match frame {
        TerminalClientFrame::Input { data } => {
            let writer = runtime.writer.clone();
            if spawn_blocking(move || -> anyhow::Result<()> {
                let mut writer = writer
                    .lock()
                    .map_err(|_| anyhow::anyhow!("terminal writer lock poisoned"))?;
                writer.write_all(data.as_bytes())?;
                writer.flush()?;
                Ok(())
            })
            .await
            .ok()
            .and_then(Result::ok)
            .is_none()
            {
                return false;
            }
        }
        TerminalClientFrame::Resize { cols, rows } => {
            let master = runtime.master.clone();
            if spawn_blocking(move || -> anyhow::Result<()> {
                let master = master
                    .lock()
                    .map_err(|_| anyhow::anyhow!("terminal master lock poisoned"))?;
                master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
                Ok(())
            })
            .await
            .ok()
            .and_then(Result::ok)
            .is_none()
            {
                return false;
            }
        }
        TerminalClientFrame::Close => {
            return false;
        }
    }
    true
}

fn spawn_terminal_reader(
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    tx: mpsc::Sender<TerminalSessionEvent>,
) -> JoinHandle<()> {
    spawn_blocking(move || {
        let reader_result = {
            let master = match master.lock() {
                Ok(master) => master,
                Err(_) => {
                    let _ = tx.blocking_send(TerminalSessionEvent::Error(
                        "Terminal session lock failed".to_string(),
                    ));
                    return;
                }
            };
            master.try_clone_reader()
        };

        let mut reader = match reader_result {
            Ok(reader) => reader,
            Err(err) => {
                let _ = tx.blocking_send(TerminalSessionEvent::Error(format!(
                    "Failed to open terminal output: {err}"
                )));
                return;
            }
        };

        let mut buf = vec![0_u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if tx
                        .blocking_send(TerminalSessionEvent::Output(text))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    let _ = tx.blocking_send(TerminalSessionEvent::Error(format!(
                        "Terminal output failed: {err}"
                    )));
                    return;
                }
            }
        }
    })
}

fn spawn_terminal_waiter(
    child: Arc<Mutex<Box<dyn Child + Send>>>,
    tx: mpsc::Sender<TerminalSessionEvent>,
) -> JoinHandle<()> {
    spawn_blocking(move || {
        let status = {
            let mut child = match child.lock() {
                Ok(child) => child,
                Err(_) => {
                    let _ = tx.blocking_send(TerminalSessionEvent::Error(
                        "Terminal child lock failed".to_string(),
                    ));
                    return;
                }
            };
            child.wait()
        };

        match status {
            Ok(status) => {
                let _ =
                    tx.blocking_send(TerminalSessionEvent::Exit(Some(status.exit_code() as i32)));
            }
            Err(err) => {
                let _ = tx.blocking_send(TerminalSessionEvent::Error(format!(
                    "Terminal session failed: {err}"
                )));
            }
        }
    })
}

fn spawn_terminal_process(spec: &TerminalSessionSpec) -> anyhow::Result<TerminalSessionRuntime> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let command = build_terminal_command(spec);
    let child = pair.slave.spawn_command(command)?;
    if let Some(pid) = child.process_id() {
        crate::process::reset_oom_score(pid);
    }
    let writer = pair.master.take_writer()?;

    Ok(TerminalSessionRuntime {
        child: Arc::new(Mutex::new(child)),
        master: Arc::new(Mutex::new(pair.master)),
        writer: Arc::new(Mutex::new(writer)),
        reader_task: spawn_blocking(|| {}),
        wait_task: spawn_blocking(|| {}),
    })
}

fn build_terminal_command(spec: &TerminalSessionSpec) -> CommandBuilder {
    match spec {
        TerminalSessionSpec::Server { shell_user } => {
            let (program, args) = build_host_shell_command(shell_user);
            command_builder(program, &args)
        }
        TerminalSessionSpec::Container { container_id } => {
            let (program, args) = build_container_shell_command(container_id);
            command_builder(program, &args)
        }
    }
}

fn command_builder(program: &str, args: &[String]) -> CommandBuilder {
    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg);
    }
    command
}

fn build_host_shell_command(shell_user: &str) -> (&'static str, Vec<String>) {
    (
        "su",
        vec![
            "-l".to_string(),
            shell_user.to_string(),
            "-s".to_string(),
            "/bin/sh".to_string(),
        ],
    )
}

fn build_container_shell_command(container_id: &str) -> (&'static str, Vec<String>) {
    (
        "docker",
        vec![
            "exec".to_string(),
            "-it".to_string(),
            container_id.to_string(),
            "sh".to_string(),
            "-lc".to_string(),
            "if command -v /bin/bash >/dev/null 2>&1; then exec /bin/bash; elif command -v /bin/ash >/dev/null 2>&1; then exec /bin/ash; else exec /bin/sh; fi".to_string(),
        ],
    )
}

async fn shutdown_terminal_session(runtime: TerminalSessionRuntime) {
    let child = runtime.child.clone();
    let _ = spawn_blocking(move || {
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    })
    .await;

    runtime.reader_task.abort();
    runtime.wait_task.abort();
}

async fn collect_terminal_targets(
    db: &sqlx::SqlitePool,
    shell_user: &str,
    host_enabled: bool,
) -> Result<Vec<TerminalTargetResponse>, AppError> {
    let mut targets = vec![TerminalTargetResponse {
        id: "server".to_string(),
        kind: TerminalTargetKind::Server,
        label: "Server".to_string(),
        shell_user: Some(shell_user.to_string()),
        enabled: host_enabled,
        requires_step_up: true,
    }];

    let apps: Vec<TerminalWorkloadRow> = sqlx::query_as(
		"SELECT id, name, container_id, status FROM apps WHERE container_id IS NOT NULL AND status = 'running' ORDER BY name ASC",
	)
	.fetch_all(db)
	.await?;

    for app in apps {
        targets.push(TerminalTargetResponse {
            id: app.id,
            kind: TerminalTargetKind::App,
            label: format!("App: {}", app.name),
            shell_user: None,
            enabled: true,
            requires_step_up: false,
        });
    }

    let databases: Vec<TerminalWorkloadRow> = sqlx::query_as(
		"SELECT id, name, container_id, status FROM databases WHERE container_id IS NOT NULL AND status = 'running' ORDER BY name ASC",
	)
	.fetch_all(db)
	.await?;

    for database in databases {
        targets.push(TerminalTargetResponse {
            id: database.id,
            kind: TerminalTargetKind::Database,
            label: format!("Database: {}", database.name),
            shell_user: None,
            enabled: true,
            requires_step_up: false,
        });
    }

    Ok(targets)
}

async fn resolve_terminal_session(
    db: &sqlx::SqlitePool,
    target_kind: &TerminalTargetKind,
    target_id: &str,
    shell_user: &str,
) -> Result<TerminalSessionSpec, TondError> {
    match target_kind {
        TerminalTargetKind::Server => {
            if target_id != "server" {
                return Err(TondError::NotFound(format!("terminal target: {target_id}")));
            }
            Ok(TerminalSessionSpec::Server {
                shell_user: shell_user.to_string(),
            })
        }
        TerminalTargetKind::App => {
            let row: Option<TerminalWorkloadRow> = sqlx::query_as(
                "SELECT id, name, container_id, status FROM apps WHERE id = ? LIMIT 1",
            )
            .bind(target_id)
            .fetch_optional(db)
            .await
            .map_err(TondError::DatabaseError)?;

            let row =
                row.ok_or_else(|| TondError::NotFound(format!("terminal target: {target_id}")))?;
            resolve_container_target(row, target_id)
        }
        TerminalTargetKind::Database => {
            let row: Option<TerminalWorkloadRow> = sqlx::query_as(
                "SELECT id, name, container_id, status FROM databases WHERE id = ? LIMIT 1",
            )
            .bind(target_id)
            .fetch_optional(db)
            .await
            .map_err(TondError::DatabaseError)?;

            let row =
                row.ok_or_else(|| TondError::NotFound(format!("terminal target: {target_id}")))?;
            resolve_container_target(row, target_id)
        }
    }
}

fn resolve_container_target(
    row: TerminalWorkloadRow,
    target_id: &str,
) -> Result<TerminalSessionSpec, TondError> {
    if row.status != "running" {
        return Err(TondError::NotFound(format!("terminal target: {target_id}")));
    }

    let container_id = row
        .container_id
        .ok_or_else(|| TondError::NotFound(format!("terminal target: {target_id}")))?;

    Ok(TerminalSessionSpec::Container { container_id })
}

fn terminal_shell_user_from_env() -> Result<String, AppError> {
    let value = env::var("TOND_TERMINAL_USER").unwrap_or_else(|_| "tond".to_string());
    validate_terminal_shell_user(&value).map_err(AppError)
}

fn validate_terminal_shell_user(value: &str) -> Result<String, TondError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(TondError::InvalidInput(
            "TOND_TERMINAL_USER must not be empty".to_string(),
        ));
    }
    if trimmed == "root" {
        return Err(TondError::InvalidInput(
            "TOND_TERMINAL_USER must not be root".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(TondError::InvalidInput(
            "TOND_TERMINAL_USER contains invalid characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

async fn send_terminal_error(socket: &mut WebSocket, message: &str) -> anyhow::Result<()> {
    socket
        .send(Message::Text(
            json!({ "type": "error", "message": message })
                .to_string()
                .into(),
        ))
        .await?;
    Ok(())
}

async fn close_socket(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, extract::State, response::IntoResponse};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tond_common::Role;
    use tond_common::ServerSettings;

    use crate::auth::RateLimiter;

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db).await.unwrap_or_else(|err| {
            panic!("failed to run migrations for terminal handler tests: {err}")
        });

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_app(
        state: &AppState,
        id: &str,
        name: &str,
        status: &str,
        container_id: Option<&str>,
    ) {
        sqlx::query(
			"INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, status, env_vars, memory_limit, cpu_limit, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, auto_deploy, watch_branch, project_id, github_app_id, registry_credential_id, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		)
		.bind(id)
		.bind(name)
		.bind(Option::<String>::None)
		.bind("git")
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(3000_i64)
		.bind(container_id)
		.bind(status)
		.bind("{}")
		.bind(Option::<i64>::None)
		.bind(Option::<i64>::None)
		.bind(Option::<String>::None)
		.bind(30_i64)
		.bind(3_i64)
		.bind(0_i64)
		.bind("[]")
		.bind(30_i64)
		.bind("dockerfile")
		.bind(0_i64)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(0_i64)
		.bind("2026-01-01T00:00:00Z")
		.bind("2026-01-01T00:00:00Z")
		.execute(&state.db)
		.await
		.unwrap_or_else(|err| panic!("failed to seed app: {err}"));
    }

    async fn seed_database(
        state: &AppState,
        id: &str,
        name: &str,
        status: &str,
        container_id: Option<&str>,
    ) {
        sqlx::query(
			"INSERT INTO databases (id, name, db_type, project_id, container_id, host_port, status, db_name, db_user, db_password, root_password, volume_path, memory_limit, custom_external_url, custom_internal_url, image_used, custom_image, created_at, updated_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		)
		.bind(id)
		.bind(name)
		.bind("postgres")
		.bind(Option::<String>::None)
		.bind(container_id)
		.bind(Option::<i64>::None)
		.bind(status)
		.bind(Some(format!("tond_{name}")))
		.bind(Some(format!("tond_{name}")))
		.bind(Some("secret".to_string()))
		.bind(Option::<String>::None)
		.bind(Some(format!("/opt/tond/data/{name}")))
		.bind(Option::<i64>::None)
		.bind(Option::<String>::None)
		.bind(Option::<String>::None)
		.bind(Some("postgres:16-alpine".to_string()))
		.bind(Option::<String>::None)
		.bind("2026-01-01T00:00:00Z")
		.bind("2026-01-01T00:00:00Z")
		.execute(&state.db)
		.await
		.unwrap_or_else(|err| panic!("failed to seed database: {err}"));
    }

    #[test]
    fn validate_terminal_shell_user_rejects_root_and_invalid_names() {
        assert_eq!(
            validate_terminal_shell_user("tond")
                .unwrap_or_else(|err| panic!("expected tond to be a valid terminal user: {err}")),
            "tond"
        );
        assert!(
            matches!(
                validate_terminal_shell_user("root"),
                Err(TondError::InvalidInput(_))
            ),
            "terminal shell user must reject root to avoid granting a root shell through the dashboard"
        );
        assert!(
            matches!(
                validate_terminal_shell_user("bad user"),
                Err(TondError::InvalidInput(_))
            ),
            "terminal shell user should reject whitespace and shell-special characters"
        );
    }

    #[test]
    fn build_terminal_commands_target_expected_shells() {
        assert_eq!(
            build_host_shell_command("tond"),
            (
                "su",
                vec![
                    "-l".to_string(),
                    "tond".to_string(),
                    "-s".to_string(),
                    "/bin/sh".to_string()
                ]
            ),
            "host sessions should switch to the configured non-root service user"
        );
        assert_eq!(
            build_container_shell_command("abc123"),
            (
                "docker",
                vec![
                    "exec".to_string(),
                    "-it".to_string(),
                    "abc123".to_string(),
                    "sh".to_string(),
                    "-lc".to_string(),
                    "if command -v /bin/bash >/dev/null 2>&1; then exec /bin/bash; elif command -v /bin/ash >/dev/null 2>&1; then exec /bin/ash; else exec /bin/sh; fi".to_string()
                ]
            ),
            "container sessions should prefer bash, then ash, then sh for minimal images"
        );
    }

    #[tokio::test]
    async fn list_targets_returns_server_and_running_workloads_only() {
        let state = test_state().await;
        seed_app(&state, "app-running", "api", "running", Some("cont-api")).await;
        seed_app(
            &state,
            "app-stopped",
            "worker",
            "stopped",
            Some("cont-worker"),
        )
        .await;
        seed_database(&state, "db-running", "postgres", "running", Some("cont-db")).await;
        seed_database(&state, "db-pending", "redis", "pending", Some("cont-redis")).await;

        let response = list_targets(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
        )
        .await
        .unwrap_or_else(|err| {
            panic!(
                "list_targets should succeed for authenticated requests: {}",
                err.0
            )
        })
        .into_response();
        let body = response_json(response).await;
        let targets: Vec<TerminalTargetResponse> =
            serde_json::from_value(body.get("data").cloned().unwrap_or_else(|| {
                panic!("expected API response to include a data field: {body}")
            }))
            .unwrap_or_else(|err| panic!("failed to decode terminal targets: {err}"));

        assert_eq!(
            targets.len(),
            3,
            "expected server, one running app, and one running database target"
        );
        assert_eq!(
            targets[0].kind,
            TerminalTargetKind::Server,
            "server target should always be listed first"
        );
        assert_eq!(
            targets[0].shell_user.as_deref(),
            Some("tond"),
            "server target should expose the configured non-root shell user"
        );
        assert!(
            targets
                .iter()
                .any(|target| target.kind == TerminalTargetKind::App && target.id == "app-running"),
            "running app containers should be selectable terminal targets"
        );
        assert!(
            targets
                .iter()
                .any(|target| target.kind == TerminalTargetKind::Database
                    && target.id == "db-running"),
            "running managed databases should be selectable terminal targets"
        );
        assert!(
            targets
                .iter()
                .all(|target| target.id != "app-stopped" && target.id != "db-pending"),
            "stopped or pending workloads should not appear in the selectable terminal target list"
        );
    }

    #[tokio::test]
    async fn resolve_terminal_session_requires_running_container_targets() {
        let state = test_state().await;
        seed_app(&state, "app-running", "api", "running", Some("cont-api")).await;
        seed_database(&state, "db-stopped", "postgres", "stopped", Some("cont-db")).await;

        match resolve_terminal_session(&state.db, &TerminalTargetKind::App, "app-running", "tond")
            .await
            .unwrap_or_else(|err| {
                panic!("running app should resolve to a container terminal session: {err}")
            }) {
            TerminalSessionSpec::Container { container_id } => assert_eq!(container_id, "cont-api"),
            TerminalSessionSpec::Server { .. } => {
                panic!("expected a container session for the running app target")
            }
        }

        assert!(
            matches!(
                resolve_terminal_session(
                    &state.db,
                    &TerminalTargetKind::Database,
                    "db-stopped",
                    "tond"
                )
                .await,
                Err(TondError::NotFound(_))
            ),
            "non-running containers should be rejected so the dashboard cannot open a dead terminal session"
        );
    }

    // ── Tickets, host-terminal switch, step-up ──

    const DASH_ORIGIN: &str = "https://dash.example.com";

    async fn dashboard_state() -> AppState {
        let state = test_state().await;
        state.settings.write().await.dashboard_domain = "dash.example.com".to_string();
        state
    }

    async fn seed_user(state: &AppState, id: &str, role: &str) -> String {
        let hash = argon2_hash("ValidPass123!");
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, ?, 'now', 'now')",
        )
        .bind(id)
        .bind(format!("{id}@example.com"))
        .bind(hash)
        .bind(role)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));

        let token = generate_raw_key();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, ?, '2999-01-01T00:00:00Z', 'now')",
        )
        .bind(hash_key(&token))
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session: {err}"));
        token
    }

    fn argon2_hash(password: &str) -> String {
        use argon2::{Argon2, PasswordHasher};
        Argon2::default()
            .hash_password(password.as_bytes())
            .unwrap_or_else(|err| panic!("hash: {err}"))
            .to_string()
    }

    fn browser_headers(token: &str, origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("__Host-tond_session={token}")
                .parse()
                .unwrap_or_else(|err| panic!("{err}")),
        );
        if let Some(origin) = origin {
            headers.insert(
                header::ORIGIN,
                origin.parse().unwrap_or_else(|err| panic!("{err}")),
            );
        }
        headers
    }

    fn ticket_request(
        kind: TerminalTargetKind,
        id: &str,
        password: Option<&str>,
    ) -> Json<TerminalTicketRequest> {
        Json(TerminalTicketRequest {
            target_kind: kind,
            target_id: id.to_string(),
            password: password.map(str::to_string),
            code: None,
        })
    }

    async fn request_ticket(
        state: &AppState,
        headers: HeaderMap,
        req: Json<TerminalTicketRequest>,
    ) -> (StatusCode, serde_json::Value) {
        let response = create_ticket(
            RateLimit,
            ClientIp(None),
            State(state.clone()),
            headers,
            req,
        )
        .await;
        let status = response.status();
        (status, response_json(response).await)
    }

    async fn enable_host_terminal(state: &AppState) {
        let response = set_host_terminal(
            LocalPeer(true),
            State(state.clone()),
            Json(HostTerminalSetting { enabled: true }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn host_terminal_is_off_by_default_and_only_the_socket_can_switch_it() {
        let state = dashboard_state().await;
        assert!(
            !host_terminal_enabled(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );

        let remote = set_host_terminal(
            LocalPeer(false),
            State(state.clone()),
            Json(HostTerminalSetting { enabled: true }),
        )
        .await;
        assert_eq!(
            remote.status(),
            StatusCode::FORBIDDEN,
            "dashboard sessions and API tokens must not be able to enable the host terminal"
        );
        assert!(
            !host_terminal_enabled(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );

        enable_host_terminal(&state).await;
        assert!(
            host_terminal_enabled(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"))
        );
        let action: String = sqlx::query_scalar("SELECT action FROM audit_log")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(action, "host_terminal_enabled");
    }

    #[tokio::test]
    async fn host_ticket_is_refused_while_the_host_terminal_is_off() {
        let state = dashboard_state().await;
        let token = seed_user(&state, "owner", "admin").await;
        let (status, body) = request_ticket(
            &state,
            browser_headers(&token, Some(DASH_ORIGIN)),
            ticket_request(TerminalTargetKind::Server, "server", Some("ValidPass123!")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains(HOST_TERMINAL_ENABLE_COMMAND),
            "the error must tell the user how to turn it on"
        );
    }

    #[tokio::test]
    async fn host_ticket_requires_step_up_password() {
        let state = dashboard_state().await;
        enable_host_terminal(&state).await;
        let token = seed_user(&state, "owner", "admin").await;

        for password in [None, Some("wrong")] {
            let (status, _) = request_ticket(
                &state,
                browser_headers(&token, Some(DASH_ORIGIN)),
                ticket_request(TerminalTargetKind::Server, "server", password),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        let (status, body) = request_ticket(
            &state,
            browser_headers(&token, Some(DASH_ORIGIN)),
            ticket_request(TerminalTargetKind::Server, "server", Some("ValidPass123!")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["data"]["expires_in"], serde_json::json!(30));
    }

    #[tokio::test]
    async fn tickets_are_refused_to_api_keys_members_and_foreign_origins() {
        let state = dashboard_state().await;
        seed_app(&state, "app-1", "api", "running", Some("cont-api")).await;
        let admin = seed_user(&state, "owner", "admin").await;
        let member = seed_user(&state, "member", "member").await;

        let mut api_key = browser_headers(&admin, Some(DASH_ORIGIN));
        api_key.insert(
            header::AUTHORIZATION,
            "Bearer k".parse().unwrap_or_else(|err| panic!("{err}")),
        );
        let (status, _) = request_ticket(
            &state,
            api_key,
            ticket_request(TerminalTargetKind::App, "app-1", None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "API tokens must be refused");

        let (status, _) = request_ticket(
            &state,
            browser_headers(&member, Some(DASH_ORIGIN)),
            ticket_request(TerminalTargetKind::App, "app-1", None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "members must be refused");

        for origin in [None, Some("https://pr-1.dash.example.com")] {
            let (status, _) = request_ticket(
                &state,
                browser_headers(&admin, origin),
                ticket_request(TerminalTargetKind::App, "app-1", None),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "origin {origin:?} must be refused"
            );
        }

        // Container exec needs no step-up.
        let (status, _) = request_ticket(
            &state,
            browser_headers(&admin, Some(DASH_ORIGIN)),
            ticket_request(TerminalTargetKind::App, "app-1", None),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn tickets_are_single_use_and_bound_to_their_origin() {
        let state = dashboard_state().await;
        seed_app(&state, "app-1", "api", "running", Some("cont-api")).await;
        let admin = seed_user(&state, "owner", "admin").await;

        let issue = || async {
            let (status, body) = request_ticket(
                &state,
                browser_headers(&admin, Some(DASH_ORIGIN)),
                ticket_request(TerminalTargetKind::App, "app-1", None),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            body["data"]["ticket"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };

        let ticket = issue().await;
        let wrong_origin = redeem_ticket(&state, &ticket, Some("http://localhost:5173")).await;
        assert!(matches!(wrong_origin, Err(ref r) if r.status() == StatusCode::FORBIDDEN));
        // A failed redemption still burns the ticket.
        assert!(
            redeem_ticket(&state, &ticket, Some(DASH_ORIGIN))
                .await
                .is_err()
        );

        let ticket = issue().await;
        let grant = redeem_ticket(&state, &ticket, Some(DASH_ORIGIN))
            .await
            .unwrap_or_else(|r| panic!("redeem failed: {}", r.status()));
        assert_eq!(grant.user.id, "owner");
        assert_eq!(grant.kind, TerminalTargetKind::App);
        let reuse = redeem_ticket(&state, &ticket, Some(DASH_ORIGIN)).await;
        assert!(matches!(reuse, Err(ref r) if r.status() == StatusCode::UNAUTHORIZED));

        let ticket = issue().await;
        sqlx::query("UPDATE terminal_tickets SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert!(
            redeem_ticket(&state, &ticket, Some(DASH_ORIGIN))
                .await
                .is_err(),
            "expired tickets must be refused"
        );
        let stored: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM terminal_tickets WHERE ticket_hash = ?")
                .bind(&ticket)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(stored, 0, "tickets are stored hashed");
    }

    #[tokio::test]
    async fn host_ticket_is_refused_at_connect_time_if_switched_off_meanwhile() {
        let state = dashboard_state().await;
        enable_host_terminal(&state).await;
        let admin = seed_user(&state, "owner", "admin").await;
        let (_, body) = request_ticket(
            &state,
            browser_headers(&admin, Some(DASH_ORIGIN)),
            ticket_request(TerminalTargetKind::Server, "server", Some("ValidPass123!")),
        )
        .await;
        let ticket = body["data"]["ticket"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let off = set_host_terminal(
            LocalPeer(true),
            State(state.clone()),
            Json(HostTerminalSetting { enabled: false }),
        )
        .await;
        assert_eq!(off.status(), StatusCode::OK);
        let redeemed = redeem_ticket(&state, &ticket, Some(DASH_ORIGIN)).await;
        assert!(matches!(redeemed, Err(ref r) if r.status() == StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn list_targets_reports_host_terminal_state() {
        let state = test_state().await;
        let response = list_targets(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
        )
        .await
        .unwrap_or_else(|err| panic!("{}", err.0))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"][0]["enabled"], serde_json::json!(false));
        assert_eq!(body["data"][0]["requires_step_up"], serde_json::json!(true));

        enable_host_terminal(&state).await;
        let response = list_targets(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
        )
        .await
        .unwrap_or_else(|err| panic!("{}", err.0))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"][0]["enabled"], serde_json::json!(true));
    }
}
