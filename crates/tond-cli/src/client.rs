// client.rs — typed HTTP client for the Tond API (/api/v1)
//
// TondClient wraps reqwest::Client and provides one async method per API
// endpoint. Every request goes through one path (`send` → `parse_envelope`):
//   - the Authorization: Bearer <token> header when an API token is set
//   - an Idempotency-Key header on mutating requests (POST/PUT/PATCH/DELETE):
//     the global --idempotency-key / TOND_IDEMPOTENCY_KEY when given, else a
//     fresh UUID per request. The same key is reused on retries.
//   - retries of transient failures: a connection error before any response,
//     502/503/504, and 409 IDEMPOTENCY_KEY_IN_USE — up to two more attempts
//     (0.5 s, then 1.5 s later). Other 4xx are never retried. Mutating
//     requests always carry a key, so a retry cannot apply an action twice.
//   - the envelope `{"success":true,"data":T}` becomes `Ok(T)`; a failure
//     becomes a `CliError` (error.rs) carrying the server's code, message,
//     hint and field, whose exit code comes from the shared catalog. A reply
//     that is not an envelope (a proxy's HTML 502) is reported with its
//     status; a connection failure is SERVER_UNREACHABLE (exit 8).
//
// Nothing here prints: commands decide how to render results (output.rs).
//
// Transports:
//   TondClient::local(socket) talks to tond-server over its root-only Unix
//   socket, where no credentials are needed (the CLI's default).
//   TondClient::new(url, token) talks HTTP(S) and sends the API token
//   (`tond_pat_…`, created with `tond token create` or in the dashboard).
//
// push_deploy():
//   Shells out to `tar czf` to package the working directory, then sends the
//   bytes as multipart/form-data to POST /api/v1/apps/{id}/upload. The server
//   extracts the tarball and queues a build; the reply carries the deployment id.
//
// ws_messages() / log_stream():
//   Open a WebSocket (e.g. /api/v1/apps/{id}/logs/stream); the token, if any,
//   travels as `?token=`. Callers read and render the frames.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio_tungstenite::tungstenite::Message;
use tond_common::{
    ApiErrorBody, AppResponse, AuditLogPage, AuditVerifyReport, BackupConfigResponse,
    CreateAppRequest, CreateCronJobRequest, CreateDatabaseRequest, CreateTokenRequest,
    CreatedDatabaseResponse, CreatedTokenResponse, CronJobResponse, CronJobRun,
    DatabaseCredentialsResponse, DeployKeyResponse, DeploymentResponse, IntegrationResponse,
    ManagedDatabaseDetail, ManagedDatabaseResponse, PasswordLinkResponse,
    PreviewDeploymentResponse, ProjectPreviewResponse, ProjectResponse, ServerSettings,
    SnapshotResponse, StorageProviderResponse, TemplateCatalogResponse, TemplateDeployRequest,
    TemplateDeployResponse, TokenResponse, UpdateCronJobRequest, UpdateSettingsRequest,
    UserResponse, WebhookConfigResponse, WebhookSecretResponse, code,
};

use crate::error::CliError;

/// Prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";

/// Header carrying the idempotency key of a mutating request.
pub const IDEMPOTENCY_HEADER: &str = "Idempotency-Key";

/// Pauses before the second and third attempt of a transient failure.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(500), Duration::from_millis(1500)];

/// Body and response of `/api/v1/settings/host-terminal`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HostTerminalSetting {
    enabled: bool,
}

pub struct TondClient {
    base_url: String,
    client: reqwest::Client,
    api_key: Option<String>,
    /// Set when talking to tond-server over its local Unix socket.
    socket_path: Option<PathBuf>,
    /// Caller-chosen idempotency key (--idempotency-key); else one per request.
    idempotency_key: Option<String>,
    retry_delays: Vec<Duration>,
}

/// Incoming messages of a WebSocket opened with `TondClient::ws_messages`.
pub type WsMessages = std::pin::Pin<
    Box<dyn futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Send>,
>;

/// Host used in request URLs sent over the Unix socket (the socket, not DNS, picks the peer).
const LOCAL_BASE_URL: &str = "http://localhost";

/// Hint shown when the local socket can't be used.
const LOCAL_HINT: &str = "Run the CLI on the Tond server as root, or connect remotely with \
                          --server <url> --token <tond_pat_…>.";

fn is_mutating(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether a response is worth another attempt.
fn is_transient(status: StatusCode, body: &str) -> bool {
    match status.as_u16() {
        502..=504 => true,
        409 => serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/code")
                    .and_then(Value::as_str)
                    .map(|c| c == code::IDEMPOTENCY_KEY_IN_USE)
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// Turn a reply into the envelope's data (`None` for `data: null`) or a
/// typed error.
pub fn parse_envelope<T: DeserializeOwned>(status: StatusCode, body: &str) -> Result<Option<T>> {
    let snippet = || {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            String::new()
        } else {
            format!(": {}", trimmed.chars().take(300).collect::<String>())
        }
    };
    let envelope = serde_json::from_str::<Value>(body)
        .ok()
        .filter(|v| v.get("success").and_then(Value::as_bool).is_some());
    let Some(mut envelope) = envelope else {
        if status.is_success() && body.trim().is_empty() {
            return Ok(None);
        }
        let message = if status.is_success() {
            format!(
                "Unexpected reply from the server (HTTP {status}){}",
                snippet()
            )
        } else {
            format!("The server answered HTTP {status}{}", snippet())
        };
        return Err(if status.is_success() {
            CliError::invalid_response(message)
        } else {
            CliError::status_only(status.as_u16(), message)
        }
        .into());
    };

    if envelope.get("success").and_then(Value::as_bool) == Some(true) {
        return match envelope.get_mut("data").map(Value::take) {
            None | Some(Value::Null) => Ok(None),
            Some(data) => serde_json::from_value(data).map(Some).map_err(|e| {
                CliError::invalid_response(format!("Could not read the server's reply: {e}")).into()
            }),
        };
    }

    let error = envelope.get_mut("error").map(Value::take);
    Err(match error {
        Some(obj @ Value::Object(_)) => match serde_json::from_value::<ApiErrorBody>(obj) {
            Ok(body) => CliError::api(status.as_u16(), body),
            Err(_) => CliError::status_only(
                status.as_u16(),
                format!("The request failed (HTTP {status})"),
            ),
        },
        // Servers before /api/v1 sent the message as a plain string.
        Some(Value::String(message)) => CliError::status_only(status.as_u16(), message),
        _ => CliError::status_only(
            status.as_u16(),
            format!("The request failed without details (HTTP {status})"),
        ),
    }
    .into())
}

impl TondClient {
    /// Client for a server reached over HTTP(S), authenticated with the API token `api_key`.
    pub fn new(base_url: &str, api_key: Option<&str>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            api_key: api_key.map(|s| s.to_string()),
            socket_path: None,
            idempotency_key: None,
            retry_delays: RETRY_DELAYS.to_vec(),
        }
    }

    /// Client for the tond-server on this machine, via its root-only Unix socket.
    pub fn local(socket_path: &Path) -> Result<Self> {
        // Probe the socket first so a missing server or missing root privileges
        // produce an actionable message instead of a generic request error.
        if let Err(err) = std::os::unix::net::UnixStream::connect(socket_path) {
            let error = match err.kind() {
                std::io::ErrorKind::NotFound => CliError::unreachable(format!(
                    "tond-server socket not found at {}. Is tond-server running?",
                    socket_path.display()
                )),
                std::io::ErrorKind::PermissionDenied => {
                    CliError::socket_permission_denied(format!(
                        "permission denied on {} (the socket is root-only).",
                        socket_path.display()
                    ))
                }
                _ => CliError::unreachable(format!(
                    "could not connect to {}: {err}",
                    socket_path.display()
                )),
            };
            return Err(error.with_hint(LOCAL_HINT).into());
        }

        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .build()
            .context("building Unix socket HTTP client")?;
        Ok(Self {
            base_url: LOCAL_BASE_URL.to_string(),
            client,
            api_key: None,
            socket_path: Some(socket_path.to_path_buf()),
            idempotency_key: None,
            retry_delays: RETRY_DELAYS.to_vec(),
        })
    }

    /// Use `key` as the Idempotency-Key of every mutating request.
    pub fn with_idempotency_key(mut self, key: Option<String>) -> Self {
        self.idempotency_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    #[cfg(test)]
    fn with_retry_delays(mut self, delays: Vec<Duration>) -> Self {
        self.retry_delays = delays;
        self
    }

    /// Add the Authorization header if an API token is configured.
    fn auth_header(&self, req: RequestBuilder) -> RequestBuilder {
        if let Some(ref key) = self.api_key {
            req.header("Authorization", format!("Bearer {}", key))
        } else {
            req
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{API_PREFIX}{path}", self.base_url)
    }

    /// Build the WebSocket URL (ws:// or wss://) from the HTTP base URL.
    fn ws_url(&self, path: &str) -> String {
        let base = self
            .base_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{}{}", base, path)
    }

    /// Send `method path` (under /api/v1) with retries; `build` adds the body.
    /// Returns the final status and body text.
    async fn send(
        &self,
        method: Method,
        path: &str,
        build: &(dyn Fn(RequestBuilder) -> Result<RequestBuilder> + Send + Sync),
    ) -> Result<(StatusCode, String)> {
        let key = is_mutating(&method).then(|| {
            self.idempotency_key
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        });
        let mut attempt = 0usize;
        loop {
            let mut req = self.auth_header(self.client.request(method.clone(), self.url(path)));
            if let Some(key) = &key {
                req = req.header(IDEMPOTENCY_HEADER, key);
            }
            let req = build(req)?;
            let retry_after = self.retry_delays.get(attempt).copied();
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.map_err(|e| {
                        CliError::unreachable(format!("The connection to tond-server broke: {e}"))
                    })?;
                    if let Some(delay) = retry_after
                        && is_transient(status, &body)
                    {
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, body));
                }
                Err(e) if e.is_connect() => {
                    if let Some(delay) = retry_after {
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(CliError::unreachable(format!(
                        "Could not connect to tond-server at {}: {e}",
                        self.base_url
                    ))
                    .with_hint("Check that tond-server is running and --server is right.")
                    .into());
                }
                Err(e) => {
                    return Err(CliError::unreachable(format!(
                        "The request to tond-server failed: {e}"
                    ))
                    .into());
                }
            }
        }
    }

    /// Request with an optional JSON body; the envelope's data, if any.
    async fn call_opt<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<T>> {
        let (status, text) = self
            .send(method, path, &move |req| {
                Ok(match &body {
                    Some(b) => req.json(b),
                    None => req,
                })
            })
            .await?;
        parse_envelope(status, &text)
    }

    /// Request whose reply must carry data.
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        self.call_opt(method, path, body)
            .await?
            .ok_or_else(|| CliError::invalid_response("No data returned").into())
    }

    /// Request returning a list (`data: null` reads as empty).
    async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        Ok(self
            .call_opt(Method::GET, path, None)
            .await?
            .unwrap_or_default())
    }

    /// Request whose data is returned as-is (`null` when absent).
    async fn value(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        Ok(self
            .call_opt(method, path, body)
            .await?
            .unwrap_or(Value::Null))
    }

    fn to_body<B: serde::Serialize>(body: &B) -> Result<Option<Value>> {
        Ok(Some(serde_json::to_value(body)?))
    }

    /// GET /api/v1/templates — the one-click catalog.
    pub async fn list_templates(&self) -> Result<TemplateCatalogResponse> {
        self.call(Method::GET, "/templates", None).await
    }

    /// POST /api/v1/templates/{id}/deploy — create the app and queue its first deploy.
    pub async fn deploy_template(
        &self,
        id: &str,
        body: &TemplateDeployRequest,
    ) -> Result<TemplateDeployResponse> {
        self.call(
            Method::POST,
            &format!("/templates/{id}/deploy"),
            Self::to_body(body)?,
        )
        .await
    }

    pub async fn list_apps(&self) -> Result<Vec<AppResponse>> {
        self.list("/apps").await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_app(
        &self,
        name: &str,
        source_type: &str,
        source_url: Option<String>,
        branch: Option<String>,
        container_port: i64,
        env_vars: Option<Map<String, Value>>,
        project_id: Option<String>,
        domain: Option<String>,
        memory_limit: Option<i64>,
        cpu_limit: Option<i64>,
        health_check_path: Option<String>,
        health_check_interval: Option<i64>,
        health_failure_threshold: Option<i64>,
        drain_timeout: Option<i64>,
        build_method: Option<String>,
    ) -> Result<AppResponse> {
        let req = CreateAppRequest {
            name: name.to_string(),
            source_type: source_type.to_string(),
            source_url,
            branch,
            container_port,
            env_vars: env_vars.map(Value::Object),
            domain,
            project_id,
            memory_limit,
            cpu_limit,
            health_check_path,
            health_check_interval,
            health_failure_threshold,
            drain_timeout,
            build_method,
            auto_deploy: false,
            watch_branch: None,
            volumes: None,
            registry_credential_id: None,
            github_app_id: None,
            dockerfile_content: None,
            compose_content: None,
            base_dir: None,
            static_build: false,
            readiness: None,
            ready_after_secs: None,
        };
        self.call(Method::POST, "/apps", Self::to_body(&req)?).await
    }

    pub async fn get_app(&self, id: &str) -> Result<AppResponse> {
        self.call(Method::GET, &format!("/apps/{id}"), None).await
    }

    /// Queue a deploy; returns the server's answer (`deployment_id`, `status`,
    /// `queue_position`, …).
    pub async fn deploy_app(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/apps/{id}/deploy"), None)
            .await
    }

    /// GET /api/v1/apps/{id}/deployments/{deployment_id} (deployment + log).
    pub async fn get_deployment(&self, id: &str, deployment_id: &str) -> Result<Value> {
        self.call(
            Method::GET,
            &format!("/apps/{id}/deployments/{deployment_id}"),
            None,
        )
        .await
    }

    /// Open a WebSocket at `path` (relative to /api/v1, over the Unix socket
    /// or TCP) and return its incoming messages. The token, if any, travels
    /// as `?token=`.
    pub async fn ws_messages(&self, path: &str) -> Result<WsMessages> {
        let path = format!("{API_PREFIX}{path}");
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = match self.api_key.as_deref() {
            Some(token) => self.ws_url(&format!("{path}{sep}token={token}")),
            None => self.ws_url(&path),
        };
        let unreachable = |e: &dyn std::fmt::Display| {
            CliError::unreachable(format!("Failed to open the stream: {e}"))
        };
        // A refused upgrade (401/403/404…) is an API error, not an outage.
        let handshake = |e: tokio_tungstenite::tungstenite::Error| match e {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                let status = resp.status();
                let body = resp
                    .body()
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default()
                    .into_owned();
                parse_envelope::<Value>(status, &body)
                    .err()
                    .unwrap_or_else(|| {
                        CliError::status_only(
                            status.as_u16(),
                            format!("The server refused the stream (HTTP {status})"),
                        )
                        .into()
                    })
            }
            other => unreachable(&other).into(),
        };
        Ok(match &self.socket_path {
            Some(socket_path) => {
                let stream = tokio::net::UnixStream::connect(socket_path)
                    .await
                    .map_err(|e| unreachable(&e))?;
                let (ws, _) = tokio_tungstenite::client_async(url.as_str(), stream)
                    .await
                    .map_err(handshake)?;
                Box::pin(ws.split().1)
            }
            None => {
                let (ws, _) = tokio_tungstenite::connect_async(&url)
                    .await
                    .map_err(handshake)?;
                Box::pin(ws.split().1)
            }
        })
    }

    /// Live log stream of an app: a `snapshot` frame, then `deploy` and
    /// `container` frames until the server closes.
    pub async fn log_stream(&self, id: &str) -> Result<WsMessages> {
        self.ws_messages(&format!("/apps/{id}/logs/stream")).await
    }

    /// Package `dir` and upload it; returns the server's answer (the queued
    /// deployment, like `deploy_app`).
    pub async fn push_deploy(&self, id: &str, dir: &str) -> Result<Value> {
        // Create a tarball of the directory, respecting .tondignore and
        // applying sensible default exclusions.
        let tar_path = format!("/tmp/tond-push-{}.tar.gz", uuid::Uuid::new_v4());

        // Build the exclude list: hard-coded defaults + .tondignore entries
        let default_excludes = [
            ".git",
            "node_modules",
            "target",
            ".svelte-kit",
            ".next",
            "__pycache__",
            ".venv",
            "dist",
            "build",
            ".DS_Store",
        ];

        let mut excludes: Vec<String> = default_excludes.iter().map(|s| s.to_string()).collect();

        // Parse .tondignore if it exists
        let tondignore_path = format!("{}/.tondignore", dir);
        if let Ok(contents) = tokio::fs::read_to_string(&tondignore_path).await {
            for line in contents.lines() {
                let trimmed = line.trim();
                // Skip empty lines and comments
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Strip trailing slashes for tar --exclude compatibility
                    let pattern = trimmed.trim_end_matches('/').to_string();
                    if !excludes.contains(&pattern) {
                        excludes.push(pattern);
                    }
                }
            }
        }

        // Build the tar command with all exclude arguments
        let mut args: Vec<String> = vec![
            "czf".to_string(),
            tar_path.clone(),
            "-C".to_string(),
            dir.to_string(),
        ];
        for exc in &excludes {
            args.push(format!("--exclude=./{}", exc));
        }
        args.push(".".to_string());

        let output = tokio::process::Command::new("tar")
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!("Failed to create tarball");
        }

        let file_bytes = tokio::fs::read(&tar_path).await?;
        let _ = tokio::fs::remove_file(&tar_path).await;

        let (status, body) = self
            .send(Method::POST, &format!("/apps/{id}/upload"), &|req| {
                let part = reqwest::multipart::Part::bytes(file_bytes.clone())
                    .file_name("upload.tar.gz")
                    .mime_str("application/gzip")?;
                Ok(req.multipart(reqwest::multipart::Form::new().part("file", part)))
            })
            .await?;
        upload_result(status, &body)
    }

    pub async fn action(&self, id: &str, action: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/apps/{id}/{action}"), None)
            .await
    }

    pub async fn delete_app(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/apps/{id}"), None)
            .await
    }

    pub async fn get_logs(&self, id: &str) -> Result<Value> {
        self.call(Method::GET, &format!("/apps/{id}/logs"), None)
            .await
    }

    /// Set individual env vars. Variables that are not named are left unchanged.
    pub async fn update_env(&self, id: &str, env_vars: Value) -> Result<Value> {
        self.value(
            Method::PATCH,
            &format!("/apps/{id}/env"),
            Some(json!({ "set": env_vars })),
        )
        .await
    }

    pub async fn update_domain(&self, id: &str, domain: &str) -> Result<Value> {
        self.value(
            Method::PUT,
            &format!("/apps/{id}/domain"),
            Some(json!({ "domain": domain })),
        )
        .await
    }

    pub async fn update_health(
        &self,
        id: &str,
        path: Option<&str>,
        interval: i64,
        threshold: i64,
    ) -> Result<Value> {
        self.value(
            Method::PUT,
            &format!("/apps/{id}/health"),
            Some(json!({
                "health_check_path": path,
                "health_check_interval": interval,
                "health_failure_threshold": threshold,
            })),
        )
        .await
    }

    /// Add a volume. The server stores it in `<data>/volumes/<app>/<name>`;
    /// `name` defaults to one derived from the container path.
    pub async fn add_volume(
        &self,
        id: &str,
        container_path: &str,
        name: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({ "container_path": container_path });
        if let Some(name) = name {
            body["name"] = Value::String(name.to_string());
        }
        self.value(Method::POST, &format!("/apps/{id}/volumes"), Some(body))
            .await
    }

    pub async fn remove_volume(&self, id: &str, container_path: &str) -> Result<Value> {
        self.value(
            Method::DELETE,
            &format!("/apps/{id}/volumes"),
            Some(json!({ "container_path": container_path })),
        )
        .await
    }

    pub async fn update_drain_timeout(&self, id: &str, seconds: i64) -> Result<Value> {
        self.value(
            Method::PUT,
            &format!("/apps/{id}/drain-timeout"),
            Some(json!({ "drain_timeout": seconds })),
        )
        .await
    }

    pub async fn list_deployments(&self, id: &str) -> Result<Vec<DeploymentResponse>> {
        self.list(&format!("/apps/{id}/deployments")).await
    }

    pub async fn rollback_app(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/apps/{id}/rollback"), None)
            .await
    }

    /// Generate (or regenerate) an Ed25519 deploy key for an app.
    /// Returns the public key string.
    pub async fn generate_deploy_key(&self, id: &str) -> Result<String> {
        let key: DeployKeyResponse = self
            .call(Method::POST, &format!("/apps/{id}/deploy-key"), None)
            .await?;
        Ok(key.public_key)
    }

    /// Get the current public deploy key for an app.
    pub async fn get_deploy_key(&self, id: &str) -> Result<String> {
        let key: DeployKeyResponse = self
            .call(Method::GET, &format!("/apps/{id}/deploy-key"), None)
            .await?;
        Ok(key.public_key)
    }

    /// Webhook URLs for an app (the secret is revealed separately).
    pub async fn get_webhook_config(&self, id: &str) -> Result<WebhookConfigResponse> {
        self.call(Method::GET, &format!("/apps/{id}/webhook"), None)
            .await
    }

    /// Replace the webhook secret with a caller-chosen value.
    pub async fn set_webhook_secret(&self, id: &str, secret: &str) -> Result<()> {
        self.value(
            Method::PUT,
            &format!("/apps/{id}/webhook-secret"),
            Some(json!({ "secret": secret })),
        )
        .await?;
        Ok(())
    }

    /// Reveal the app's webhook secret (admin only, audit-logged).
    pub async fn reveal_webhook_secret(&self, id: &str) -> Result<WebhookSecretResponse> {
        self.call(Method::GET, &format!("/apps/{id}/webhook-secret"), None)
            .await
    }

    /// Replace the webhook secret with a freshly generated one; returns it.
    pub async fn rotate_webhook_secret(&self, id: &str) -> Result<WebhookSecretResponse> {
        self.call(
            Method::POST,
            &format!("/apps/{id}/webhook-secret/rotate"),
            None,
        )
        .await
    }

    /// Enable or disable auto-deploy, optionally setting the watch branch.
    pub async fn set_auto_deploy(
        &self,
        id: &str,
        auto_deploy: bool,
        watch_branch: Option<&str>,
    ) -> Result<()> {
        self.value(
            Method::PUT,
            &format!("/apps/{id}/auto-deploy"),
            Some(json!({
                "auto_deploy": auto_deploy,
                "watch_branch": watch_branch,
            })),
        )
        .await?;
        Ok(())
    }

    // ── Preview Deployments ──

    /// Create a preview deployment for an app. Returns the preview row immediately
    /// (status = "building"); the actual build runs in the background.
    pub async fn create_preview(
        &self,
        id: &str,
        ttl_hours: Option<i64>,
    ) -> Result<PreviewDeploymentResponse> {
        self.call(
            Method::POST,
            &format!("/apps/{id}/preview"),
            Some(json!({ "ttl_hours": ttl_hours })),
        )
        .await
    }

    /// List active (non-expired) preview deployments for an app.
    pub async fn list_previews(&self, id: &str) -> Result<Vec<PreviewDeploymentResponse>> {
        self.list(&format!("/apps/{id}/previews")).await
    }

    /// Stop and remove a specific preview deployment.
    pub async fn delete_preview(&self, id: &str, preview_id: &str) -> Result<Value> {
        self.value(
            Method::DELETE,
            &format!("/apps/{id}/previews/{preview_id}"),
            None,
        )
        .await
    }

    // ── Projects ──

    pub async fn list_projects(&self) -> Result<Vec<ProjectResponse>> {
        self.list("/projects").await
    }

    pub async fn create_project(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<ProjectResponse> {
        self.call(
            Method::POST,
            "/projects",
            Some(json!({ "name": name, "description": description })),
        )
        .await
    }

    pub async fn get_project(&self, id: &str) -> Result<ProjectResponse> {
        self.call(Method::GET, &format!("/projects/{id}"), None)
            .await
    }

    pub async fn delete_project(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/projects/{id}"), None)
            .await
    }

    pub async fn deploy_project(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/projects/{id}/deploy"), None)
            .await
    }

    /// Assign an app to a project (or remove it when project_id is None).
    pub async fn assign_app_project(
        &self,
        app_id: &str,
        project_id: Option<&str>,
    ) -> Result<Value> {
        self.value(
            Method::PUT,
            &format!("/apps/{app_id}/project"),
            Some(json!({ "project_id": project_id })),
        )
        .await
    }

    // ── Project-level Previews ──

    /// Create a project preview (one preview deployment per member app).
    /// Returns the project preview row immediately with status "building".
    pub async fn create_project_preview(
        &self,
        project_id: &str,
        ttl_hours: Option<i64>,
    ) -> Result<ProjectPreviewResponse> {
        self.call(
            Method::POST,
            &format!("/projects/{project_id}/previews"),
            Some(json!({ "ttl_hours": ttl_hours })),
        )
        .await
    }

    /// List active project previews for a project.
    pub async fn list_project_previews(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectPreviewResponse>> {
        self.list(&format!("/projects/{project_id}/previews")).await
    }

    /// Delete (stop + expire) a project preview.
    pub async fn delete_project_preview(
        &self,
        project_id: &str,
        preview_id: &str,
    ) -> Result<Value> {
        self.value(
            Method::DELETE,
            &format!("/projects/{project_id}/previews/{preview_id}"),
            None,
        )
        .await
    }

    // ── Managed Databases ──

    pub async fn list_databases(&self) -> Result<Vec<ManagedDatabaseResponse>> {
        self.list("/databases").await
    }

    pub async fn create_database(
        &self,
        name: &str,
        db_type: &str,
        project_id: Option<&str>,
        memory_limit: Option<i64>,
    ) -> Result<CreatedDatabaseResponse> {
        let body = CreateDatabaseRequest {
            name: name.to_string(),
            db_type: db_type.to_string(),
            project_id: project_id.map(|s| s.to_string()),
            memory_limit,
            ..Default::default()
        };
        self.call(Method::POST, "/databases", Self::to_body(&body)?)
            .await
    }

    /// Reveal a database's passwords and connection strings (admin only,
    /// audit-logged).
    pub async fn get_database_credentials(&self, id: &str) -> Result<DatabaseCredentialsResponse> {
        self.call(Method::GET, &format!("/databases/{id}/credentials"), None)
            .await
    }

    pub async fn get_database(&self, id: &str) -> Result<ManagedDatabaseDetail> {
        self.call(Method::GET, &format!("/databases/{id}"), None)
            .await
    }

    pub async fn delete_database(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/databases/{id}"), None)
            .await
    }

    pub async fn stop_database(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/databases/{id}/stop"), None)
            .await
    }

    pub async fn start_database(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/databases/{id}/start"), None)
            .await
    }

    // ── Cron Jobs ──

    pub async fn list_cron_jobs(&self) -> Result<Vec<CronJobResponse>> {
        self.list("/cron").await
    }

    pub async fn create_cron_job(
        &self,
        name: &str,
        schedule: &str,
        image: &str,
        command: Option<&[String]>,
        project_id: Option<&str>,
    ) -> Result<CronJobResponse> {
        let body = CreateCronJobRequest {
            name: name.to_string(),
            schedule: schedule.to_string(),
            image: image.to_string(),
            command: command.map(|c| c.to_vec()),
            project_id: project_id.map(|s| s.to_string()),
            timeout_seconds: None,
            env: None,
            concurrency_policy: None,
            memory_mb: None,
            cpu_shares: None,
            timezone: None,
            max_retries: None,
            retry_delay_seconds: None,
            volumes: None,
        };
        self.call(Method::POST, "/cron", Self::to_body(&body)?)
            .await
    }

    pub async fn get_cron_job(&self, id: &str) -> Result<CronJobResponse> {
        self.call(Method::GET, &format!("/cron/{id}"), None).await
    }

    pub async fn update_cron_job(
        &self,
        id: &str,
        schedule: Option<&str>,
        image: Option<&str>,
        command: Option<&[String]>,
        enabled: Option<bool>,
    ) -> Result<CronJobResponse> {
        let body = UpdateCronJobRequest {
            name: None,
            schedule: schedule.map(|s| s.to_string()),
            image: image.map(|s| s.to_string()),
            command: command.map(|c| c.to_vec()),
            enabled,
            timeout_seconds: None,
            env: None,
            env_set: Default::default(),
            env_unset: vec![],
            project_id: None,
            concurrency_policy: None,
            memory_mb: None,
            cpu_shares: None,
            timezone: None,
            max_retries: None,
            retry_delay_seconds: None,
            volumes: None,
        };
        self.call(Method::PATCH, &format!("/cron/{id}"), Self::to_body(&body)?)
            .await
    }

    pub async fn delete_cron_job(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/cron/{id}"), None)
            .await
    }

    pub async fn trigger_cron_job(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/cron/{id}/trigger"), None)
            .await
    }

    pub async fn list_cron_job_runs(&self, id: &str) -> Result<Vec<CronJobRun>> {
        self.list(&format!("/cron/{id}/runs")).await
    }

    // ── Password Management ──

    /// Change the current user's password.
    /// The server verifies `current_password` with argon2, then re-hashes
    /// `new_password` and invalidates all other active sessions.
    pub async fn change_password(&self, current_password: &str, new_password: &str) -> Result<()> {
        self.value(
            Method::POST,
            "/auth/change-password",
            Some(json!({
                "current_password": current_password,
                "new_password": new_password
            })),
        )
        .await?;
        Ok(())
    }

    // ── Settings ──

    /// Fetch the current server-wide settings.
    pub async fn get_settings(&self) -> Result<ServerSettings> {
        self.call(Method::GET, "/settings", None).await
    }

    /// Patch one or more server-wide settings. Only supplied fields are updated.
    pub async fn update_settings(&self, req_body: UpdateSettingsRequest) -> Result<ServerSettings> {
        self.call(Method::PUT, "/settings", Self::to_body(&req_body)?)
            .await
    }

    /// Whether the dashboard's host terminal is switched on.
    pub async fn get_host_terminal(&self) -> Result<bool> {
        let s: HostTerminalSetting = self
            .call(Method::GET, "/settings/host-terminal", None)
            .await?;
        Ok(s.enabled)
    }

    /// Switch the dashboard's host terminal on or off. The server accepts this
    /// only over the local Unix socket.
    pub async fn set_host_terminal(&self, enabled: bool) -> Result<bool> {
        let s: HostTerminalSetting = self
            .call(
                Method::PUT,
                "/settings/host-terminal",
                Self::to_body(&HostTerminalSetting { enabled })?,
            )
            .await?;
        Ok(s.enabled)
    }

    // ── Notification Integrations ──

    /// List all notification integrations.
    pub async fn list_integrations(&self) -> Result<Vec<IntegrationResponse>> {
        self.list("/integrations").await
    }

    /// Get a single integration by ID.
    pub async fn get_integration(&self, id: &str) -> Result<IntegrationResponse> {
        self.call(Method::GET, &format!("/integrations/{id}"), None)
            .await
    }

    /// Delete an integration.
    pub async fn delete_integration(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/integrations/{id}"), None)
            .await
    }

    /// Send a test notification through an integration.
    pub async fn test_integration(&self, id: &str) -> Result<String> {
        let data = self
            .value(Method::POST, &format!("/integrations/{id}/test"), None)
            .await?;
        Ok(data
            .get("message")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| "Test sent".into()))
    }

    /// Enable or disable an integration.
    pub async fn set_integration_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<IntegrationResponse> {
        self.call(
            Method::PATCH,
            &format!("/integrations/{id}"),
            Some(json!({ "enabled": enabled })),
        )
        .await
    }

    // ── Storage providers ─────────────────────────────────────────────────────

    pub async fn list_storage_providers(&self) -> Result<Vec<StorageProviderResponse>> {
        self.list("/storage-providers").await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_storage_provider(
        &self,
        name: &str,
        endpoint: &str,
        region: &str,
        bucket: &str,
        prefix: Option<&str>,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<StorageProviderResponse> {
        self.call(
            Method::POST,
            "/storage-providers",
            Some(json!({
                "name": name,
                "kind": "s3",
                "config": {
                    "endpoint": endpoint,
                    "region": region,
                    "bucket": bucket,
                    "prefix": prefix.unwrap_or(""),
                    "access_key_id": access_key_id,
                    "secret_access_key": secret_access_key,
                }
            })),
        )
        .await
    }

    pub async fn test_storage_provider(&self, id: &str) -> Result<String> {
        let data = self
            .value(Method::POST, &format!("/storage-providers/{id}/test"), None)
            .await?;
        Ok(data
            .get("message")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| "Connection successful".into()))
    }

    pub async fn delete_storage_provider(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/storage-providers/{id}"), None)
            .await
    }

    // ── Snapshots ─────────────────────────────────────────────────────────────

    /// Take a full snapshot. Returns once the local copy is complete; a remote
    /// upload (when configured) continues in the background.
    pub async fn create_snapshot(&self, name: Option<&str>) -> Result<SnapshotResponse> {
        self.call(Method::POST, "/snapshots", Some(json!({ "name": name })))
            .await
    }

    pub async fn list_snapshots(&self, app: Option<&str>) -> Result<Vec<SnapshotResponse>> {
        match app {
            Some(a) => {
                self.list(&format!("/snapshots?app={}", urlencoding::encode(a)))
                    .await
            }
            None => self.list("/snapshots").await,
        }
    }

    pub async fn delete_snapshot(&self, id: &str) -> Result<Value> {
        self.value(Method::DELETE, &format!("/snapshots/{id}"), None)
            .await
    }

    /// Restore a full snapshot; the answer names the pre-restore snapshot
    /// (`pre_restore_snapshot_id`).
    pub async fn restore_snapshot_full(&self, id: &str) -> Result<Value> {
        self.value(Method::POST, &format!("/snapshots/{id}/restore"), None)
            .await
    }

    pub async fn restore_app_from_snapshot(
        &self,
        snapshot_id: &str,
        app_name: &str,
    ) -> Result<Value> {
        self.value(
            Method::POST,
            &format!("/snapshots/{snapshot_id}/restore-app"),
            Some(json!({ "app_name": app_name })),
        )
        .await
    }

    pub async fn rollback_database(&self, snapshot_id: &str, db_name: &str) -> Result<Value> {
        self.value(
            Method::POST,
            &format!("/snapshots/{snapshot_id}/restore-database"),
            Some(json!({ "db_name": db_name })),
        )
        .await
    }

    pub async fn get_backup_config(&self) -> Result<BackupConfigResponse> {
        self.call(Method::GET, "/snapshots/config", None).await
    }

    pub async fn update_backup_config(&self, body: Value) -> Result<BackupConfigResponse> {
        self.call(Method::PUT, "/snapshots/config", Some(body))
            .await
    }

    // ── API tokens ──

    /// Create an API token; the response carries the secret (shown once).
    pub async fn create_token(&self, body: &CreateTokenRequest) -> Result<CreatedTokenResponse> {
        self.call(Method::POST, "/tokens", Self::to_body(body)?)
            .await
    }

    /// List API tokens (every user's when `all`).
    pub async fn list_tokens(&self, all: bool) -> Result<Vec<TokenResponse>> {
        self.list(&format!("/tokens?all={all}")).await
    }

    /// Revoke an API token by id.
    pub async fn revoke_token(&self, id: &str) -> Result<TokenResponse> {
        self.call(Method::DELETE, &format!("/tokens/{id}"), None)
            .await
    }

    // ── Users ──

    pub async fn list_users(&self) -> Result<Vec<UserResponse>> {
        self.list("/users").await
    }

    /// Issue a one-time set-password link for a user (invite or reset).
    pub async fn create_password_link(&self, user_id: &str) -> Result<PasswordLinkResponse> {
        self.call(
            Method::POST,
            &format!("/users/{user_id}/password-link"),
            None,
        )
        .await
    }

    // ── Audit log ──

    /// Verify the audit log's hash chain.
    pub async fn verify_audit_log(&self) -> Result<AuditVerifyReport> {
        self.call(Method::GET, "/audit-log/verify", None).await
    }

    /// One page of the audit log, newest first.
    pub async fn list_audit_log(&self, query: &[(&str, String)]) -> Result<AuditLogPage> {
        let qs: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
            .collect();
        self.call(Method::GET, &format!("/audit-log?{}", qs.join("&")), None)
            .await
    }
}

/// Turn the upload endpoint's reply into the queued deployment. The server
/// refuses unsafe or oversized archives with a structured error that says
/// what to change; anything else (a proxy's HTML page, an empty body) is
/// reported with its status.
fn upload_result(status: StatusCode, body: &str) -> Result<Value> {
    Ok(parse_envelope::<Value>(status, body)?.unwrap_or(Value::Null))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Clone, Default)]
    struct CaptureState {
        auth_header: Arc<Mutex<Option<String>>>,
        body: Arc<Mutex<Option<Value>>>,
    }

    pub(crate) async fn spawn_test_server(router: Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|err| panic!("failed to bind test server listener: {err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("failed to read test server address: {err}"));
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|err| panic!("test server failed: {err}"));
        });
        addr
    }

    pub(crate) fn app_response_json() -> Value {
        json!({
            "id": "app-1",
            "name": "demo",
            "domain": null,
            "source_type": "git",
            "source_url": "https://example.com/repo.git",
            "status": "running",
            "container_port": 3000,
            "host_port": 18080,
            "memory_limit": 268435456,
            "cpu_limit": 500000000,
            "health_check_path": null,
            "health_check_interval": 30,
            "health_failure_threshold": 3,
            "health_consecutive_failures": 0,
            "volumes": [],
            "drain_timeout": 30,
            "build_method": "tbuild",
            "auto_deploy": false,
            "watch_branch": null,
            "deploy_key_pub": null,
            "project_id": null,
            "registry_credential_id": null,
            "github_app_id": null,
            "gitlab_token_id": null,
            "preview_enabled": false,
            "preview_shared_keys": [],
            "preview_env_vars": {},
            "dockerfile_content": null,
            "compose_content": null,
            "base_dir": null,
            "static_build": false,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        })
    }

    #[test]
    fn new_trims_trailing_slash_from_base_url() {
        let client = TondClient::new("https://tond.example.com/", None);

        assert_eq!(
            client.base_url, "https://tond.example.com",
            "TondClient::new should trim trailing slashes from the base URL"
        );
    }

    #[test]
    fn ws_url_converts_http_and_https_schemes() {
        let http_client = TondClient::new("http://localhost:3001", None);
        assert_eq!(
            http_client.ws_url("/api/v1/apps/demo/logs/stream"),
            "ws://localhost:3001/api/v1/apps/demo/logs/stream",
            "ws_url should convert http:// base URLs to ws://"
        );

        let https_client = TondClient::new("https://tond.example.com", None);
        assert_eq!(
            https_client.ws_url("/api/v1/apps/demo/logs/stream"),
            "wss://tond.example.com/api/v1/apps/demo/logs/stream",
            "ws_url should convert https:// base URLs to wss://"
        );
    }

    #[test]
    fn auth_header_adds_bearer_token_only_when_configured() {
        let client_with_key = TondClient::new("http://localhost:3001", Some("secret"));
        let request_with_key = client_with_key
            .auth_header(
                client_with_key
                    .client
                    .get("http://localhost:3001/api/v1/apps"),
            )
            .build()
            .unwrap_or_else(|err| panic!("failed to build authorized request: {err}"));
        assert_eq!(
            request_with_key
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer secret"),
            "auth_header should attach the configured bearer token"
        );

        let client_without_key = TondClient::new("http://localhost:3001", None);
        let request_without_key = client_without_key
            .auth_header(
                client_without_key
                    .client
                    .get("http://localhost:3001/api/v1/apps"),
            )
            .build()
            .unwrap_or_else(|err| panic!("failed to build unauthenticated request: {err}"));
        assert_eq!(
            request_without_key.headers().get("authorization"),
            None,
            "auth_header should leave requests untouched when no API token is configured"
        );
    }

    #[tokio::test]
    async fn local_client_talks_to_server_over_unix_socket_without_api_key() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let socket_path = dir.path().join("tond.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path)
            .unwrap_or_else(|err| panic!("failed to bind test Unix socket: {err}"));
        let state = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps",
                get(
                    |State(state): State<CaptureState>, headers: HeaderMap| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        axum::Json(json!({ "success": true, "data": [], "error": null }))
                    },
                ),
            )
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|err| panic!("test server failed: {err}"));
        });

        let client = TondClient::local(&socket_path)
            .unwrap_or_else(|err| panic!("expected local client to connect: {err}"));
        let apps = client
            .list_apps()
            .await
            .unwrap_or_else(|err| panic!("expected list_apps over the socket to succeed: {err}"));

        assert!(apps.is_empty());
        assert_eq!(
            *state.auth_header.lock().await,
            None,
            "the local socket client should not send credentials"
        );
    }

    #[test]
    fn local_client_reports_missing_socket() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let socket_path = dir.path().join("missing.sock");

        let error = TondClient::local(&socket_path)
            .err()
            .unwrap_or_else(|| panic!("connecting to a missing socket should fail"));

        assert!(
            error.to_string().contains("socket not found"),
            "a missing socket should produce an actionable message, got: {error}"
        );
    }

    #[tokio::test]
    async fn list_apps_returns_empty_vec_when_success_response_has_no_data() {
        let router = Router::new().route(
            "/api/v1/apps",
            get(|| async { axum::Json(json!({ "success": true, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let apps = client
            .list_apps()
            .await
            .unwrap_or_else(|err| panic!("expected list_apps to succeed: {err}"));

        assert!(
            apps.is_empty(),
            "list_apps should fall back to an empty vector when the API omits data on success"
        );
    }

    #[tokio::test]
    async fn list_apps_returns_unknown_error_when_api_fails_without_message() {
        let router = Router::new().route(
            "/api/v1/apps",
            get(|| async { axum::Json(json!({ "success": false, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let error = client
            .list_apps()
            .await
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected list_apps to fail"));

        assert_eq!(
            error.to_string(),
            "The request failed without details (HTTP 200 OK)",
            "list_apps should surface a fallback error message when the API does not provide one"
        );
    }

    #[tokio::test]
    async fn get_app_requires_data_in_successful_response() {
        let router = Router::new().route(
            "/api/v1/apps/app-1",
            get(|| async { axum::Json(json!({ "success": true, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let error = client
            .get_app("app-1")
            .await
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected get_app to fail"));

        assert_eq!(
            error.to_string(),
            "No data returned",
            "get_app should fail when the API reports success but omits the app payload"
        );
    }

    #[tokio::test]
    async fn create_app_sends_expected_payload_and_authorization_header() {
        let capture = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps",
                post(
                    |State(state): State<CaptureState>, headers: HeaderMap, body: Bytes| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        *state.body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode create_app request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": app_response_json(),
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), Some("secret-key"));

        let response = client
            .create_app(
                "demo",
                "git",
                Some("https://example.com/repo.git".to_string()),
                Some("main".to_string()),
                3000,
                None,
                None,
                Some("demo.example.com".to_string()),
                Some(512),
                Some(123456789),
                Some("/health".to_string()),
                Some(15),
                Some(5),
                Some(45),
                Some("dockerfile".to_string()),
            )
            .await
            .unwrap_or_else(|err| panic!("expected create_app to succeed: {err}"));

        assert_eq!(
            response.name, "demo",
            "create_app should return the deserialized app payload on success"
        );

        let auth_header = capture.auth_header.lock().await.clone();
        assert_eq!(
            auth_header.as_deref(),
            Some("Bearer secret-key"),
            "create_app should attach the bearer token when an API token is configured"
        );

        let body = capture
            .body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected create_app request body to be captured"));
        assert_eq!(
            body.get("name"),
            Some(&json!("demo")),
            "create_app should send the requested app name"
        );
        assert_eq!(
            body.get("source_type"),
            Some(&json!("git")),
            "create_app should send the requested source type"
        );
        assert_eq!(
            body.get("auto_deploy"),
            Some(&json!(false)),
            "create_app should default auto_deploy to false"
        );
        assert_eq!(
            body.get("static_build"),
            Some(&json!(false)),
            "create_app should default static_build to false"
        );
        assert_eq!(
            body.get("build_method"),
            Some(&json!("dockerfile")),
            "create_app should forward the requested build method"
        );
    }

    #[tokio::test]
    async fn create_app_sends_image_template_payload_without_build_method() {
        let capture = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps",
                post(
                    |State(state): State<CaptureState>, headers: HeaderMap, body: Bytes| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        *state.body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode create_app request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": app_response_json(),
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        client
            .create_app(
                "analytics",
                "image",
                Some("ghcr.io/plausible/community-edition".to_string()),
                None,
                8000,
                None,
                None,
                None,
                None,
                None,
                Some("/".to_string()),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_or_else(|err| panic!("expected image template create_app to succeed: {err}"));

        let body = capture
            .body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected create_app request body to be captured"));
        assert_eq!(
            body.get("source_type"),
            Some(&json!("image")),
            "templated image creates should send image as the source type"
        );
        assert_eq!(
            body.get("source_url"),
            Some(&json!("ghcr.io/plausible/community-edition")),
            "templated image creates should send the template image reference"
        );
        assert_eq!(
            body.get("build_method"),
            Some(&Value::Null),
            "templated image creates should omit build_method so the server accepts the payload"
        );
    }

    #[tokio::test]
    async fn create_project_and_database_send_expected_payloads() {
        #[derive(Clone, Default)]
        struct MultiCaptureState {
            project_body: std::sync::Arc<tokio::sync::Mutex<Option<Value>>>,
            database_body: std::sync::Arc<tokio::sync::Mutex<Option<Value>>>,
        }

        let capture = MultiCaptureState::default();
        let router = Router::new()
            .route(
                "/api/v1/projects",
                post(
                    |State(state): State<MultiCaptureState>, body: Bytes| async move {
                        *state.project_body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode project request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": {
                                "id": "project-1",
                                "name": "analytics",
                                "description": "Shared analytics services",
                                "env_vars": {},
                                "created_at": "2026-01-01T00:00:00Z",
                                "updated_at": "2026-01-01T00:00:00Z",
                                "active_preview_count": 0
                            },
                            "error": null
                        })))
                    },
                ),
            )
            .route(
                "/api/v1/databases",
                post(
                    |State(state): State<MultiCaptureState>, body: Bytes| async move {
                        *state.database_body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode database request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": {
                                "id": "db-1",
                                "name": "analytics-db",
                                "db_type": "postgres",
                                "project_id": "project-1",
                                "host_port": 5432,
                                "status": "running",
                                "db_name": "tond_analytics",
                                "db_user": "tond_analytics",
                                "has_password": true,
                                "memory_limit": null,
                                "custom_external_url": null,
                                "custom_internal_url": null,
                                "image_used": null,
                                "custom_image": null,
                                "created_at": "2026-01-01T00:00:00Z",
                                "updated_at": "2026-01-01T00:00:00Z",
                                "connection_string": "postgres://tond_analytics:••••••••@localhost:5432/tond_analytics",
                                "internal_connection_string": "postgres://tond_analytics:••••••••@analytics-db:5432/tond_analytics",
                                "credentials": {
                                    "db_user": "tond_analytics",
                                    "db_password": "secret",
                                    "root_password": null,
                                    "connection_string": "postgres://tond_analytics:secret@localhost:5432/tond_analytics",
                                    "public_connection_string": null,
                                    "internal_connection_string": "postgres://tond_analytics:secret@analytics-db:5432/tond_analytics"
                                }
                            },
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let project = client
            .create_project("analytics", Some("Shared analytics services"))
            .await
            .unwrap_or_else(|err| panic!("expected create_project to succeed: {err}"));
        let db = client
            .create_database("analytics-db", "postgres", Some(&project.id), None)
            .await
            .unwrap_or_else(|err| panic!("expected create_database to succeed: {err}"));

        assert_eq!(
            project.id, "project-1",
            "project creation should return the server id"
        );
        assert_eq!(
            db.detail.database.project_id.as_deref(),
            Some("project-1"),
            "database creation should preserve the supplied project id"
        );

        let project_body = capture
            .project_body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected project request body to be captured"));
        let database_body = capture
            .database_body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected database request body to be captured"));

        assert_eq!(
            project_body.get("name"),
            Some(&json!("analytics")),
            "create_project should send the requested project name"
        );
        assert_eq!(
            database_body.get("project_id"),
            Some(&json!("project-1")),
            "create_database should send the created project id when attaching the dependency"
        );
    }

    #[tokio::test]
    async fn test_integration_uses_default_message_when_response_omits_one() {
        let router = Router::new().route(
            "/api/v1/integrations/integration-1/test",
            post(|| async {
                axum::Json(json!({
                    "success": true,
                    "data": {},
                    "error": null
                }))
            }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let message = client
            .test_integration("integration-1")
            .await
            .unwrap_or_else(|err| panic!("expected test_integration to succeed: {err}"));

        assert_eq!(
            message, "Test sent",
            "test_integration should fall back to a default success message when the API omits one"
        );
    }

    fn cli_error(err: &anyhow::Error) -> CliError {
        crate::error::find(err)
            .cloned()
            .unwrap_or_else(|| panic!("expected a CliError, got: {err:#}"))
    }

    #[test]
    fn structured_errors_become_typed_errors() {
        let err = parse_envelope::<Value>(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"success":false,"data":null,"error":{"code":"APP_NOT_FOUND","message":"App not found: web","hint":"List apps with: tond apps list","field":"app"}}"#,
        )
        .err()
        .unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(
            cli_error(&err),
            CliError {
                status: Some(404),
                code: Some("APP_NOT_FOUND".into()),
                message: "App not found: web".into(),
                hint: Some("List apps with: tond apps list".into()),
                field: Some("app".into()),
                exit: 3,
            }
        );
    }

    #[test]
    fn legacy_string_errors_and_non_json_replies_use_the_status() {
        let err = parse_envelope::<Value>(
            reqwest::StatusCode::CONFLICT,
            r#"{"success":false,"error":"Deploy already running"}"#,
        )
        .err()
        .unwrap_or_else(|| panic!("expected an error"));
        let e = cli_error(&err);
        assert_eq!((e.code, e.exit), (None, 5));
        assert_eq!(e.message, "Deploy already running");

        let err = upload_result(reqwest::StatusCode::BAD_GATEWAY, "<html>bad gateway</html>")
            .err()
            .unwrap_or_else(|| panic!("expected an error"));
        let e = cli_error(&err);
        assert_eq!(e.exit, 8, "a proxy 502 means the server is unavailable");
        assert!(e.message.contains("502"), "status is shown: {}", e.message);

        let err = upload_result(
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"success":false,"data":null,"error":{"code":"PAYLOAD_TOO_LARGE","message":"Too large","hint":"add build output to .tondignore"}}"#,
        )
        .err()
        .unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(
            cli_error(&err).hint.as_deref(),
            Some("add build output to .tondignore")
        );
        assert_eq!(
            upload_result(
                reqwest::StatusCode::ACCEPTED,
                r#"{"success":true,"data":{"deployment_id":"dep-1"}}"#
            )
            .ok(),
            Some(json!({"deployment_id": "dep-1"}))
        );
    }

    #[derive(Clone, Default)]
    struct RetryState {
        /// Idempotency-Key header of each request (None when absent).
        keys: Arc<Mutex<Vec<Option<String>>>>,
    }

    fn key_of(headers: &HeaderMap) -> Option<String> {
        headers
            .get(IDEMPOTENCY_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    fn fast(client: TondClient) -> TondClient {
        client.with_retry_delays(vec![Duration::from_millis(1), Duration::from_millis(1)])
    }

    #[tokio::test]
    async fn mutating_requests_retry_503_with_the_same_idempotency_key() {
        let state = RetryState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps/web/deploy",
                post(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        let mut keys = state.keys.lock().await;
                        keys.push(key_of(&headers));
                        if keys.len() == 1 {
                            (StatusCode::SERVICE_UNAVAILABLE, "busy".to_string())
                        } else {
                            (
                                StatusCode::ACCEPTED,
                                json!({"success": true, "data": {"deployment_id": "dep-1"}})
                                    .to_string(),
                            )
                        }
                    },
                ),
            )
            .with_state(state.clone());
        let addr = spawn_test_server(router).await;
        let client = fast(TondClient::new(&format!("http://{addr}"), None));

        let data = client
            .deploy_app("web")
            .await
            .unwrap_or_else(|err| panic!("expected the retry to succeed: {err}"));
        assert_eq!(data["deployment_id"], "dep-1");
        let keys = state.keys.lock().await.clone();
        assert_eq!(keys.len(), 2, "one retry after the 503");
        assert!(keys[0].is_some(), "mutating requests carry a key");
        assert_eq!(keys[0], keys[1], "the retry reuses the key");
    }

    #[tokio::test]
    async fn explicit_idempotency_key_is_sent_and_gets_carry_none() {
        let state = RetryState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps/web",
                get(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        state.keys.lock().await.push(key_of(&headers));
                        axum::Json(json!({"success": true, "data": app_response_json()}))
                    },
                )
                .delete(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        state.keys.lock().await.push(key_of(&headers));
                        axum::Json(json!({"success": true, "data": null}))
                    },
                ),
            )
            .with_state(state.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{addr}"), None)
            .with_idempotency_key(Some("ci-run-42".into()));

        client
            .get_app("web")
            .await
            .unwrap_or_else(|err| panic!("get_app: {err}"));
        client
            .delete_app("web")
            .await
            .unwrap_or_else(|err| panic!("delete_app: {err}"));
        assert_eq!(
            state.keys.lock().await.clone(),
            vec![None, Some("ci-run-42".to_string())]
        );
    }

    #[tokio::test]
    async fn key_in_use_is_retried_but_other_4xx_are_not() {
        let state = RetryState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps/busy/restart",
                post(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        state.keys.lock().await.push(key_of(&headers));
                        (
                            StatusCode::CONFLICT,
                            axum::Json(json!({"success": false, "data": null, "error": {
                                "code": "IDEMPOTENCY_KEY_IN_USE", "message": "still running"}})),
                        )
                    },
                ),
            )
            .route(
                "/api/v1/apps/gone/restart",
                post(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        state.keys.lock().await.push(key_of(&headers));
                        (
                            StatusCode::NOT_FOUND,
                            axum::Json(json!({"success": false, "data": null, "error": {
                                "code": "APP_NOT_FOUND", "message": "App not found: gone"}})),
                        )
                    },
                ),
            )
            .with_state(state.clone());
        let addr = spawn_test_server(router).await;
        let client = fast(TondClient::new(&format!("http://{addr}"), None));

        let err = client
            .action("busy", "restart")
            .await
            .err()
            .unwrap_or_else(|| panic!("expected a conflict"));
        assert_eq!(cli_error(&err).exit, 5);
        assert_eq!(state.keys.lock().await.len(), 3, "two retries");

        state.keys.lock().await.clear();
        let err = client
            .action("gone", "restart")
            .await
            .err()
            .unwrap_or_else(|| panic!("expected not found"));
        assert_eq!(cli_error(&err).exit, 3);
        assert_eq!(state.keys.lock().await.len(), 1, "404 is not retried");
    }

    #[tokio::test]
    async fn non_json_502_exhausts_retries_and_exits_8() {
        let state = RetryState::default();
        let router = Router::new()
            .route(
                "/api/v1/apps",
                get(
                    |State(state): State<RetryState>, headers: HeaderMap| async move {
                        state.keys.lock().await.push(key_of(&headers));
                        (StatusCode::BAD_GATEWAY, "<html>502 Bad Gateway</html>")
                    },
                ),
            )
            .with_state(state.clone());
        let addr = spawn_test_server(router).await;
        let client = fast(TondClient::new(&format!("http://{addr}"), None));

        let err = client
            .list_apps()
            .await
            .err()
            .unwrap_or_else(|| panic!("expected a 502"));
        let e = cli_error(&err);
        assert_eq!((e.status, e.exit), (Some(502), 8));
        assert_eq!(state.keys.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn connection_refused_is_server_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|err| panic!("bind: {err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("addr: {err}"));
        drop(listener);
        let client = fast(TondClient::new(&format!("http://{addr}"), None));

        let err = client
            .list_apps()
            .await
            .err()
            .unwrap_or_else(|| panic!("expected a connection error"));
        let e = cli_error(&err);
        assert_eq!(e.code.as_deref(), Some("SERVER_UNREACHABLE"));
        assert_eq!(e.exit, 8);
    }

    #[test]
    fn missing_socket_exits_8() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let err = TondClient::local(&dir.path().join("missing.sock"))
            .err()
            .unwrap_or_else(|| panic!("expected an error"));
        let e = cli_error(&err);
        assert_eq!(e.exit, 8);
        assert!(e.hint.is_some_and(|h| h.contains("--server")));
    }
}
