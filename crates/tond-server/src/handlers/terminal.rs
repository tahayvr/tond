use std::{
    env,
    io::{Read, Write},
    sync::{Arc, Mutex},
};

use axum::{
    Json,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, spawn_blocking},
};
use tond_common::{ApiResponse, TondError};

use crate::{
    auth::{self, AuthRole, RateLimit, Role, extractor::admin_only_message},
    error::AppError,
    handlers::auth::{extract_session_token, validate_session},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalTargetKind {
    Server,
    App,
    Database,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalTargetResponse {
    pub id: String,
    pub kind: TerminalTargetKind,
    pub label: String,
    pub shell_user: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TerminalSessionQuery {
    pub target_kind: TerminalTargetKind,
    pub target_id: String,
    pub token: Option<String>,
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
    auth: AuthRole,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let shell_user = terminal_shell_user_from_env()?;
    let mut targets = collect_terminal_targets(&state.db, &shell_user).await?;
    // The host shell is admin-only; members only see app and database containers.
    if auth.0 != Role::Admin {
        targets.retain(|t| t.kind != TerminalTargetKind::Server);
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(targets))))
}

pub async fn terminal_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<TerminalSessionQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, query, headers))
}

async fn handle_terminal_socket(
    mut socket: WebSocket,
    state: AppState,
    query: TerminalSessionQuery,
    headers: HeaderMap,
) {
    let Some(role) = authenticate(&state, &query.token, &headers).await else {
        close_socket(&mut socket, 4001, "Authentication required").await;
        return;
    };

    if query.target_kind == TerminalTargetKind::Server && role != Role::Admin {
        let _ = send_terminal_error(
            &mut socket,
            &admin_only_message("open a shell on the server"),
        )
        .await;
        close_socket(&mut socket, 4403, "Forbidden").await;
        return;
    }

    let shell_user = match terminal_shell_user_from_env() {
        Ok(user) => user,
        Err(err) => {
            let _ = send_terminal_error(&mut socket, &err.0.to_string()).await;
            close_socket(&mut socket, 4400, "Terminal is not configured").await;
            return;
        }
    };

    let spec = match resolve_terminal_session(
        &state.db,
        &query.target_kind,
        &query.target_id,
        &shell_user,
    )
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

    if socket
        .send(Message::Text(json!({ "type": "ready" }).to_string().into()))
        .await
        .is_err()
    {
        shutdown_terminal_session(runtime).await;
        return;
    }

    let (tx, mut rx) = mpsc::channel::<TerminalSessionEvent>(256);
    let reader_task = spawn_terminal_reader(runtime.master.clone(), tx.clone());
    let wait_task = spawn_terminal_waiter(runtime.child.clone(), tx);
    let runtime = TerminalSessionRuntime {
        reader_task,
        wait_task,
        ..runtime
    };

    loop {
        tokio::select! {
            output = rx.recv() => {
                match output {
                    Some(TerminalSessionEvent::Output(text)) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(TerminalSessionEvent::Exit(code)) => {
                        let _ = socket
                            .send(Message::Text(json!({
                                "type": "exit",
                                "code": code
                            }).to_string().into()))
                            .await;
                        shutdown_terminal_session(runtime).await;
                        return;
                    }
                    Some(TerminalSessionEvent::Error(message)) => {
                        let _ = send_terminal_error(&mut socket, &message).await;
                        break;
                    }
                    None => break,
                }
            }
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_terminal_client_message(&runtime, text.as_str()).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    shutdown_terminal_session(runtime).await;
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
) -> Result<Vec<TerminalTargetResponse>, AppError> {
    let mut targets = vec![TerminalTargetResponse {
        id: "server".to_string(),
        kind: TerminalTargetKind::Server,
        label: "Server".to_string(),
        shell_user: Some(shell_user.to_string()),
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

/// Authenticate the WebSocket caller and resolve their role. API keys are
/// root-equivalent (admin) until scoped tokens land in Phase 1.
async fn authenticate(
    state: &AppState,
    token: &Option<String>,
    headers: &HeaderMap,
) -> Option<Role> {
    if let Some(raw_key) = token {
        return matches!(auth::validate_key(&state.db, raw_key).await, Ok(Some(_)))
            .then_some(Role::Admin);
    }

    if let Some(token) = extract_session_token(headers)
        && let Ok(Some(user)) = validate_session(&state.db, &token).await
    {
        return Some(Role::from_user_role(&user.role));
    }

    None
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

        let response = list_targets(RateLimit, AuthRole(Role::Admin), State(state))
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
    async fn list_targets_hides_the_server_shell_from_members() {
        let state = test_state().await;
        seed_app(&state, "app-running", "api", "running", Some("cont-api")).await;

        let response = list_targets(RateLimit, AuthRole(Role::Member), State(state))
            .await
            .unwrap_or_else(|err| panic!("list_targets should succeed for members: {}", err.0))
            .into_response();
        let body = response_json(response).await;
        let targets: Vec<TerminalTargetResponse> =
            serde_json::from_value(body.get("data").cloned().unwrap_or_default())
                .unwrap_or_else(|err| panic!("failed to decode terminal targets: {err}"));

        assert!(
            targets
                .iter()
                .all(|target| target.kind != TerminalTargetKind::Server),
            "members must not be offered a host shell"
        );
        assert!(targets.iter().any(|target| target.id == "app-running"));
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
}
