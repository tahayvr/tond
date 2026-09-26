// handlers/host.rs — the System page: host history, disk, Docker, heartbeat
//
//   GET  /api/system/host               (metrics:read) current host numbers,
//                                       filesystems, disk-full forecast, the
//                                       cached disk breakdown, Docker health
//   GET  /api/system/host/history       (metrics:read) ?range=1h|24h|7d|30d
//   GET  /api/system/host/forecast      (metrics:read) disk-full forecast
//   POST /api/system/disk/refresh       (metrics:read) measure the disk
//                                       breakdown again (throttled)
//   GET  /api/system/disk/cleanup       (settings:write) what "Free up space"
//                                       would remove
//   POST /api/system/disk/cleanup       (settings:write) remove it (audited)
//   GET  /api/system/heartbeat          (metrics:read) settings (the URL is
//                                       write-only: only its host is shown)
//                                       and the last outcome
//   PUT  /api/system/heartbeat          (settings:write)
//   POST /api/system/heartbeat/test     (settings:write) send one ping now

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tond_common::{ApiResponse, TondError};

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Authz, RateLimit, scope, secret_file},
    error::AppError,
    metrics::{
        docker_host::{self, CleanupPreview, CleanupResult, DiskUsage, DockerHealth},
        host::{self, DiskEta},
        host_history::{self, HistoryRange, HostSeries},
    },
    runtime::changes,
    state::AppState,
    uptime::{self, heartbeat},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/system/host", get(get_host))
        .route("/api/system/host/history", get(get_history))
        .route("/api/system/host/forecast", get(get_forecast))
        .route("/api/system/disk/refresh", post(refresh_disk))
        .route(
            "/api/system/disk/cleanup",
            get(cleanup_preview).post(cleanup),
        )
        .route(
            "/api/system/heartbeat",
            get(get_heartbeat).put(update_heartbeat),
        )
        .route("/api/system/heartbeat/test", post(test_heartbeat))
}

fn ok<T: Serialize>(body: T) -> impl IntoResponse {
    (StatusCode::OK, Json(ApiResponse::ok(body)))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── Host ──

#[derive(Debug, Serialize)]
pub struct FilesystemResponse {
    pub mount: String,
    pub used: u64,
    pub total: u64,
    pub inodes_used: u64,
    pub inodes_total: u64,
}

#[derive(Debug, Serialize)]
pub struct HostCurrentResponse {
    pub cpu_percent: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub cpus: usize,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// The data directory's filesystem.
    pub data_fs: FilesystemResponse,
    /// `/`, when it is a different filesystem.
    pub root_fs: Option<FilesystemResponse>,
    pub collected_at: i64,
}

#[derive(Debug, Serialize)]
pub struct HostResponse {
    /// None until the collector's first sample.
    pub current: Option<HostCurrentResponse>,
    pub forecast: DiskEta,
    /// None until the first measurement (a couple of minutes after start).
    pub disk: Option<DiskUsage>,
    pub docker: DockerHealth,
}

fn current() -> Option<HostCurrentResponse> {
    let s = host::snapshot()?;
    Some(HostCurrentResponse {
        cpu_percent: s.cpu_percent,
        load1: s.load1,
        load5: s.load5,
        load15: s.load15,
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
        mem_used: s.mem_used,
        mem_total: s.mem_total,
        swap_used: s.swap_used,
        swap_total: s.swap_total,
        data_fs: FilesystemResponse {
            mount: s.disk_mount.clone(),
            used: s.disk_used,
            total: s.disk_total,
            inodes_used: s.inodes_used,
            inodes_total: s.inodes_total,
        },
        root_fs: s.root.as_ref().map(|r| FilesystemResponse {
            mount: "/".into(),
            used: r.used,
            total: r.total,
            inodes_used: r.inodes_used,
            inodes_total: r.inodes_total,
        }),
        collected_at: s.collected_at,
    })
}

pub async fn get_host(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let forecast = host::disk_full_eta(&state.db, now()).await?;
    Ok(ok(HostResponse {
        current: current(),
        forecast,
        disk: docker_host::cached_disk(),
        docker: docker_host::health(&state.docker).await,
    }))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_range")]
    pub range: String,
}

fn default_range() -> String {
    "24h".into()
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub range: &'static str,
    pub step: i64,
    pub from: i64,
    pub to: i64,
    /// `/` is a separate filesystem (root_* differs from disk_*).
    pub separate_root: bool,
    pub data_mount: String,
    pub series: HostSeries,
}

pub async fn get_history(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    let range = HistoryRange::parse(&q.range).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "range must be one of 1h, 24h, 7d, 30d (got '{}')",
            q.range
        )))
    })?;
    let to = now();
    let (from, step, series) = host_history::query(&state.db, range, to).await?;
    let snap = host::snapshot();
    Ok(ok(HistoryResponse {
        range: range.as_str(),
        step,
        from,
        to,
        separate_root: snap.as_ref().is_some_and(|s| s.root.is_some()),
        data_mount: snap.map(|s| s.disk_mount).unwrap_or_else(|| "/".into()),
        series,
    }))
}

pub async fn get_forecast(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok(ok(host::disk_full_eta(&state.db, now()).await?))
}

// ── Disk ──

pub async fn refresh_disk(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok(ok(docker_host::refresh_disk(
        &state.docker,
        &secret_file::data_dir(),
    )
    .await))
}

fn docker_error(e: anyhow::Error) -> AppError {
    AppError(TondError::Internal(format!(
        "Docker didn't answer: {e}. Check the Docker row in Services below."
    )))
}

pub async fn cleanup_preview(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let preview: CleanupPreview = docker_host::cleanup_preview(&state.docker)
        .await
        .map_err(docker_error)?;
    Ok(ok(preview))
}

pub async fn cleanup(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let result: Result<CleanupResult, _> =
        docker_host::cleanup(&state.docker, secret_file::data_dir()).await;
    let entry = AuditEntry::new("disk_cleanup").resource("system", "docker", "Docker");
    match result {
        Ok(r) => {
            record(
                &state.db,
                &auth.audit_actor(),
                entry.detail(format!(
                    "freed {} bytes: build cache {} ({} entries), {} dangling image(s) {}",
                    r.total_bytes,
                    r.build_cache_bytes,
                    r.build_cache_entries,
                    r.images_deleted,
                    r.image_bytes
                )),
            )
            .await;
            Ok(ok(r))
        }
        Err(e) => {
            record(
                &state.db,
                &auth.audit_actor(),
                entry.failed().detail(e.to_string()),
            )
            .await;
            Err(docker_error(e))
        }
    }
}

// ── Heartbeat ──

#[derive(Debug, Serialize)]
pub struct HeartbeatResponse {
    pub configured: bool,
    /// The URL's host (the URL itself is never returned).
    pub url_host: Option<String>,
    pub method: String,
    pub interval_secs: i64,
    pub enabled: bool,
    pub last_ok_at: Option<i64>,
    pub last_failure_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_skipped_at: Option<i64>,
    pub last_skip_reason: Option<String>,
    pub next_at: Option<i64>,
    /// Whether a ping would be sent right now; the reason when not.
    pub healthy: bool,
    pub unhealthy_reason: Option<String>,
}

async fn heartbeat_response(state: &AppState) -> Result<HeartbeatResponse, AppError> {
    let cfg = heartbeat::load(&state.db)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    let health = heartbeat::healthy(state).await;
    Ok(HeartbeatResponse {
        configured: cfg.url.is_some(),
        url_host: cfg.url_host(),
        next_at: cfg.active().then(uptime::next_heartbeat).flatten(),
        method: cfg.method,
        interval_secs: cfg.interval_secs,
        enabled: cfg.enabled,
        last_ok_at: cfg.last_ok_at,
        last_failure_at: cfg.last_failure_at,
        last_error: cfg.last_error,
        last_skipped_at: cfg.last_skipped_at,
        last_skip_reason: cfg.last_skip_reason,
        healthy: health.is_ok(),
        unhealthy_reason: health.err(),
    })
}

pub async fn get_heartbeat(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok(ok(heartbeat_response(&state).await?))
}

/// PUT body. `url`: absent keeps the stored URL, "" removes it.
#[derive(Debug, Deserialize)]
pub struct UpdateHeartbeatRequest {
    pub url: Option<String>,
    pub method: Option<String>,
    pub interval_secs: Option<i64>,
    pub enabled: Option<bool>,
}

pub fn validate_heartbeat(req: &UpdateHeartbeatRequest) -> Result<heartbeat::Update, String> {
    if let Some(url) = req.url.as_deref()
        && !url.trim().is_empty()
    {
        heartbeat::validate_url(url)?;
    }
    let method = match req.method.as_deref() {
        None => None,
        Some(m) if m.eq_ignore_ascii_case("GET") => Some("GET".to_string()),
        Some(m) if m.eq_ignore_ascii_case("POST") => Some("POST".to_string()),
        Some(_) => return Err("The method must be GET or POST".into()),
    };
    if let Some(i) = req.interval_secs
        && !(heartbeat::MIN_INTERVAL_SECS..=heartbeat::MAX_INTERVAL_SECS).contains(&i)
    {
        return Err("The interval must be between 1 minute and 24 hours".into());
    }
    Ok(heartbeat::Update {
        url: req.url.clone(),
        method,
        interval_secs: req.interval_secs.map(|i| i / 60 * 60),
        enabled: req.enabled,
    })
}

pub async fn update_heartbeat(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UpdateHeartbeatRequest>,
) -> Result<impl IntoResponse, AppError> {
    let update = validate_heartbeat(&req).map_err(|m| AppError(TondError::InvalidInput(m)))?;
    heartbeat::save(&state.db, &update)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    // Not a watched table: tell the uptime loop directly.
    changes::notify(changes::Table::Uptime);
    let mut what = Vec::new();
    match update.url.as_deref().map(str::trim) {
        Some("") => what.push("URL removed".to_string()),
        Some(_) => what.push("URL changed".to_string()),
        None => {}
    }
    if let Some(m) = &update.method {
        what.push(format!("method {m}"));
    }
    if let Some(i) = update.interval_secs {
        what.push(format!("every {} min", i / 60));
    }
    if let Some(e) = update.enabled {
        what.push(if e { "on" } else { "off" }.to_string());
    }
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("heartbeat_update")
            .resource("settings", "heartbeat", "Heartbeat")
            .detail(what.join(", ")),
    )
    .await;
    Ok(ok(heartbeat_response(&state).await?))
}

#[derive(Debug, Serialize)]
pub struct HeartbeatTestResponse {
    #[serde(flatten)]
    pub outcome: heartbeat::Outcome,
    pub heartbeat: HeartbeatResponse,
}

pub async fn test_heartbeat(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let cfg = heartbeat::load(&state.db)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    let url = cfg.url.as_deref().ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "Add a heartbeat URL first, then send a test ping".into(),
        ))
    })?;
    // A test ping goes out even when Tond is unhealthy: it tests the URL.
    let outcome = heartbeat::send(url, &cfg.method).await;
    let _ = heartbeat::record(&state.db, &outcome, now()).await;
    let entry = AuditEntry::new("heartbeat_test").resource("settings", "heartbeat", "Heartbeat");
    let entry = match &outcome {
        heartbeat::Outcome::Failed { error } => entry.failed().detail(error.clone()),
        _ => entry,
    };
    record(&state.db, &auth.audit_actor(), entry).await;
    Ok(ok(HeartbeatTestResponse {
        outcome,
        heartbeat: heartbeat_response(&state).await?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    async fn body(resp: impl IntoResponse) -> serde_json::Value {
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    #[test]
    fn heartbeat_updates_are_validated() {
        let r = |url: Option<&str>, method: Option<&str>, interval: Option<i64>| {
            validate_heartbeat(&UpdateHeartbeatRequest {
                url: url.map(str::to_string),
                method: method.map(str::to_string),
                interval_secs: interval,
                enabled: None,
            })
        };
        assert!(r(Some("https://hc-ping.com/x"), Some("post"), Some(300)).is_ok());
        assert_eq!(
            r(None, Some("post"), None).map(|u| u.method),
            Ok(Some("POST".into()))
        );
        assert!(r(Some(""), None, None).is_ok(), "empty removes the URL");
        assert!(r(Some("not a url"), None, None).is_err());
        assert!(r(None, Some("PUT"), None).is_err());
        assert!(r(None, None, Some(10)).is_err());
        assert_eq!(
            r(None, None, Some(125)).map(|u| u.interval_secs),
            Ok(Some(120))
        );
    }

    #[tokio::test]
    async fn host_history_and_heartbeat_endpoints_answer() {
        let state = crate::uptime::tests::test_state().await;
        let admin = || Authz::unchecked(Principal::test_user(Role::Admin));

        let v = body(
            get_history(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Query(HistoryQuery { range: "1h".into() }),
            )
            .await
            .unwrap_or_else(|e| panic!("history: {}", e.0)),
        )
        .await;
        assert_eq!(v["data"]["range"], "1h");
        assert_eq!(v["data"]["step"], 60);
        assert!(v["data"]["series"]["t"].is_array());
        let bad = get_history(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Query(HistoryQuery { range: "1y".into() }),
        )
        .await;
        assert!(bad.is_err());

        let v = body(
            get_forecast(RateLimit, admin(), State(state.clone()))
                .await
                .unwrap_or_else(|e| panic!("forecast: {}", e.0)),
        )
        .await;
        assert_eq!(v["data"]["day"]["status"], "insufficient_data");

        let v = body(
            update_heartbeat(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Json(UpdateHeartbeatRequest {
                    url: Some("https://hc-ping.com/secret-uuid".into()),
                    method: None,
                    interval_secs: Some(600),
                    enabled: Some(true),
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("put: {}", e.0)),
        )
        .await;
        let d = &v["data"];
        assert_eq!(d["configured"], true);
        assert_eq!(d["url_host"], "hc-ping.com");
        assert_eq!(d["interval_secs"], 600);
        assert_eq!(d["healthy"], false, "no Docker in tests");
        assert!(
            !v.to_string().contains("secret-uuid"),
            "the URL is write-only"
        );
        let audit: String =
            sqlx::query_scalar("SELECT detail FROM audit_log WHERE action = 'heartbeat_update'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_default();
        assert!(!audit.contains("secret-uuid"));

        let v = body(
            get_heartbeat(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
            )
            .await
            .unwrap_or_else(|e| panic!("get: {}", e.0)),
        )
        .await;
        assert_eq!(v["data"]["method"], "GET");
    }
}
