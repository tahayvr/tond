// handlers/uptime.rs — uptime checks
//
//   GET  /api/v1/uptime                     (metrics:read) every check: status and
//                                        uptime % over 24 h / 7 d / 30 d
//   GET  /api/v1/apps/{id}/uptime           (metrics:read) one app's check: status,
//                                        last probe (TLS expiry, DNS), uptime %,
//                                        a 48 × 30 min status strip, 90 daily
//                                        bars, response times and incidents
//   PUT  /api/v1/apps/{id}/uptime           (apps:write) path, interval, expected
//                                        status, timeout, on/off
//   POST /api/v1/apps/{id}/uptime/check     (metrics:read) probe now
//   GET  /api/v1/system/uptime              (metrics:read) the dashboard's own check
//   PUT  /api/v1/system/uptime              (settings:write)
//   POST /api/v1/system/uptime/check        (metrics:read)
//
// `{id}` may be the app's id or name. An app without a domain has no check
// (`check: null` with a reason).

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tond_common::{ApiResponse, TondError};

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Authz, RateLimit, scope},
    error::AppError,
    state::AppState,
    uptime::{
        self, LastProbe, dns_label,
        store::{self, Bar, CheckRow, CheckSettings, Incident, Tally},
    },
};

const INCIDENT_LIMIT: i64 = 50;

// ── Responses ──

#[derive(Debug, Serialize)]
pub struct CheckSettingsResponse {
    pub id: i64,
    pub enabled: bool,
    pub path: String,
    pub interval_secs: i64,
    pub expected_status: Option<i64>,
    pub timeout_secs: i64,
}

impl From<&CheckRow> for CheckSettingsResponse {
    fn from(c: &CheckRow) -> Self {
        Self {
            id: c.id,
            enabled: c.enabled,
            path: c.path.clone(),
            interval_secs: c.interval_secs,
            expected_status: c.expected_status,
            timeout_secs: c.timeout_secs,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct UptimePercentResponse {
    pub day: Option<f64>,
    pub week: Option<f64>,
    pub month: Option<f64>,
    pub day_probes: i64,
}

impl UptimePercentResponse {
    fn from_windows(w: &store::Windows) -> Self {
        Self {
            day: w.day.percent(),
            week: w.week.percent(),
            month: w.month.percent(),
            day_probes: w.day.probes,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LastProbeResponse {
    #[serde(flatten)]
    pub probe: LastProbe,
    /// Whole days until the certificate expires (negative when expired).
    pub cert_days_left: Option<i64>,
    pub dns_label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct IncidentResponse {
    #[serde(flatten)]
    pub incident: Incident,
    /// Seconds down (so far, for an open incident).
    pub duration_secs: i64,
}

#[derive(Debug, Default, Serialize)]
pub struct LatencyResponse {
    pub t: Vec<i64>,
    pub ms: Vec<i64>,
}

/// One check, in full.
#[derive(Debug, Serialize)]
pub struct UptimeCheckResponse {
    /// None when the app has no domain.
    pub check: Option<CheckSettingsResponse>,
    pub host: Option<String>,
    pub url: Option<String>,
    /// "up" | "down" | "paused" | "pending" (no probe yet)
    pub status: &'static str,
    pub paused_reason: Option<String>,
    pub last: Option<LastProbeResponse>,
    pub down_since: Option<i64>,
    pub next_at: Option<i64>,
    pub uptime: UptimePercentResponse,
    /// Last 24 h in 30-minute bars.
    pub recent: Vec<Bar>,
    /// Last 90 days, one bar a day.
    pub daily: Vec<Bar>,
    /// Response time of successful probes over the last 24 h (10-minute buckets).
    pub latency: LatencyResponse,
    pub incidents: Vec<IncidentResponse>,
}

/// One row of GET /api/v1/uptime.
#[derive(Debug, Serialize)]
pub struct UptimeSummaryResponse {
    pub check_id: i64,
    /// "app" | "dashboard"
    pub kind: String,
    pub app_id: Option<String>,
    pub name: String,
    pub host: String,
    pub status: &'static str,
    pub uptime: UptimePercentResponse,
    pub last_at: Option<i64>,
    pub latency_ms: Option<u32>,
    pub down_since: Option<i64>,
}

fn status_of(active: bool, live: &uptime::LiveStatus) -> &'static str {
    if !active {
        "paused"
    } else if live.down_since.is_some() {
        "down"
    } else if live.last.is_some() {
        "up"
    } else {
        "pending"
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Everything about one check.
async fn describe(
    state: &AppState,
    check_id: Option<i64>,
) -> Result<UptimeCheckResponse, AppError> {
    let now = now();
    let checks = uptime_checks(state).await?;
    let Some(check) = check_id.and_then(|id| checks.into_iter().find(|c| c.0.id == id)) else {
        return Ok(UptimeCheckResponse {
            check: None,
            host: None,
            url: None,
            status: "paused",
            paused_reason: Some(
                "The app has no domain yet. Uptime checks start once it has one.".into(),
            ),
            last: None,
            down_since: None,
            next_at: None,
            uptime: UptimePercentResponse::default(),
            recent: Vec::new(),
            daily: Vec::new(),
            latency: LatencyResponse::default(),
            incidents: Vec::new(),
        });
    };
    let (row, run) = check;
    let live = uptime::live_status(row.id);
    let rolled = uptime::rolled_until(now);
    let windows = store::windows(&state.db, now, rolled).await?;
    let recent = store::recent_bars(&state.db, row.id, now, 1800, 48).await?;
    let daily = store::daily_bars(&state.db, row.id, now, 90, rolled).await?;
    let (t, ms) = store::latency_series(&state.db, row.id, now - 86_400, 600).await?;
    let incidents = store::incidents(&state.db, row.id, INCIDENT_LIMIT)
        .await?
        .into_iter()
        .map(|i| IncidentResponse {
            duration_secs: i.ended_at.unwrap_or(now) - i.started_at,
            incident: i,
        })
        .collect();
    let status = status_of(run.active, &live);
    let host = (!run.host.is_empty()).then(|| run.host.clone());
    Ok(UptimeCheckResponse {
        url: host.as_ref().map(|h| format!("https://{h}{}", row.path)),
        host,
        status,
        paused_reason: run.paused_reason.map(str::to_string),
        last: live.last.clone().map(|probe| LastProbeResponse {
            cert_days_left: probe
                .result
                .cert_expires_at
                .map(|t| (t - now).div_euclid(86_400)),
            dns_label: dns_label(probe.result.dns),
            probe,
        }),
        down_since: live.down_since,
        next_at: live.next_at,
        uptime: windows
            .get(&row.id)
            .map(UptimePercentResponse::from_windows)
            .unwrap_or_default(),
        recent,
        daily,
        latency: LatencyResponse { t, ms },
        incidents,
        check: Some(CheckSettingsResponse::from(&row)),
    })
}

/// Stored rows paired with their runnable form.
async fn uptime_checks(state: &AppState) -> Result<Vec<(CheckRow, uptime::Check)>, AppError> {
    let rows = store::load_checks(&state.db).await?;
    let dashboard = state.settings.read().await.dashboard_domain.clone();
    let stored: Vec<CheckRow> = rows.iter().map(|r| r.check.clone()).collect();
    let built = uptime::build_checks(rows, &dashboard);
    Ok(stored
        .into_iter()
        .filter_map(|row| {
            let run = built.iter().find(|c| c.id == row.id)?.clone();
            Some((row, run))
        })
        .collect())
}

async fn app_check_id(
    state: &AppState,
    id_or_name: &str,
) -> Result<(String, String, Option<i64>), AppError> {
    let app: Option<(String, String)> =
        sqlx::query_as("SELECT id, name FROM apps WHERE id = ? OR name = ? LIMIT 1")
            .bind(id_or_name)
            .bind(id_or_name)
            .fetch_optional(&state.db)
            .await?;
    let (app_id, name) = app.ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.into())))?;
    let check = store::check_for(&state.db, "app", &app_id).await?;
    Ok((app_id, name, check.map(|c| c.id)))
}

async fn dashboard_check_id(state: &AppState) -> Result<Option<i64>, AppError> {
    Ok(store::check_for(&state.db, "dashboard", "")
        .await?
        .map(|c| c.id))
}

fn ok<T: Serialize>(body: T) -> impl IntoResponse {
    (StatusCode::OK, Json(ApiResponse::ok(body)))
}

// ── Handlers ──

pub async fn list_checks(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let now = now();
    let windows = store::windows(&state.db, now, uptime::rolled_until(now)).await?;
    let empty = store::Windows::default();
    let rows: Vec<UptimeSummaryResponse> = uptime_checks(&state)
        .await?
        .into_iter()
        .map(|(row, run)| {
            let live = uptime::live_status(row.id);
            UptimeSummaryResponse {
                check_id: row.id,
                kind: row.kind.clone(),
                app_id: run.app_id.clone(),
                name: run.name.clone(),
                host: run.host.clone(),
                status: status_of(run.active, &live),
                uptime: UptimePercentResponse::from_windows(windows.get(&row.id).unwrap_or(&empty)),
                last_at: live.last.as_ref().map(|l| l.at),
                latency_ms: live.last.as_ref().map(|l| l.result.latency_ms),
                down_since: live.down_since,
            }
        })
        .collect();
    Ok(ok(rows))
}

pub async fn get_app_check(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (_, _, check) = app_check_id(&state, &id).await?;
    Ok(ok(describe(&state, check).await?))
}

pub async fn get_dashboard_check(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let check = dashboard_check_id(&state).await?;
    Ok(ok(describe(&state, check).await?))
}

/// PUT body. Every field is required: the form sends the whole check.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct UpdateCheckRequest {
    pub enabled: bool,
    pub path: String,
    pub interval_secs: i64,
    /// null = any 2xx or 3xx
    pub expected_status: Option<i64>,
    pub timeout_secs: i64,
}

/// Validate a PUT body into stored settings.
pub fn validate(req: &UpdateCheckRequest) -> Result<CheckSettings, String> {
    let path = req.path.trim();
    if !path.starts_with('/') {
        return Err("The path must start with / (for example /health)".into());
    }
    if path.len() > 512 || path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The path can't contain spaces and must be shorter than 512 characters".into());
    }
    if !(uptime::MIN_INTERVAL_SECS..=uptime::MAX_INTERVAL_SECS).contains(&req.interval_secs)
        || req.interval_secs % 60 != 0
    {
        return Err("The interval must be a whole number of minutes, from 1 to 60".into());
    }
    if let Some(s) = req.expected_status
        && !(100..=599).contains(&s)
    {
        return Err("The expected status must be an HTTP status code (100–599)".into());
    }
    if !(1..=30).contains(&req.timeout_secs) {
        return Err("The timeout must be between 1 and 30 seconds".into());
    }
    Ok(CheckSettings {
        enabled: req.enabled,
        path: path.to_string(),
        interval_secs: req.interval_secs,
        expected_status: req.expected_status,
        timeout_secs: req.timeout_secs,
    })
}

fn describe_change(s: &CheckSettings) -> String {
    format!(
        "{} · {} every {} min · expect {} · timeout {} s",
        if s.enabled { "on" } else { "off" },
        s.path,
        s.interval_secs / 60,
        s.expected_status
            .map(|c| c.to_string())
            .unwrap_or_else(|| "2xx/3xx".into()),
        s.timeout_secs
    )
}

pub async fn update_app_check(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateCheckRequest>,
) -> Result<impl IntoResponse, AppError> {
    let settings = validate(&req).map_err(|m| AppError(TondError::InvalidInput(m)))?;
    let (app_id, name, check) = app_check_id(&state, &id).await?;
    let check = check.ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "This app has no domain, so it has no uptime check yet".into(),
        ))
    })?;
    store::update_check(&state.db, check, &settings).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("uptime_check_update")
            .resource("app", &app_id, &name)
            .detail(describe_change(&settings)),
    )
    .await;
    Ok(ok(describe(&state, Some(check)).await?))
}

pub async fn update_dashboard_check(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UpdateCheckRequest>,
) -> Result<impl IntoResponse, AppError> {
    let settings = validate(&req).map_err(|m| AppError(TondError::InvalidInput(m)))?;
    let check = dashboard_check_id(&state)
        .await?
        .ok_or_else(|| AppError(TondError::Internal("the dashboard check is missing".into())))?;
    store::update_check(&state.db, check, &settings).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("uptime_check_update")
            .resource("settings", "dashboard_uptime", "Dashboard uptime check")
            .detail(describe_change(&settings)),
    )
    .await;
    Ok(ok(describe(&state, Some(check)).await?))
}

async fn check_now(state: &AppState, check: Option<i64>) -> Result<UptimeCheckResponse, AppError> {
    let id = check.ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "There is nothing to check: the app has no domain".into(),
        ))
    })?;
    uptime::check_now(state, id)
        .await
        .map_err(|e| AppError(TondError::InvalidInput(e.to_string())))?;
    describe(state, Some(id)).await
}

pub async fn check_app_now(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (_, _, check) = app_check_id(&state, &id).await?;
    Ok(ok(check_now(&state, check).await?))
}

pub async fn check_dashboard_now(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let check = dashboard_check_id(&state).await?;
    Ok(ok(check_now(&state, check).await?))
}

/// Uptime of every check keyed by app id (for other handlers).
#[allow(dead_code)]
pub async fn app_uptime(state: &AppState) -> Result<HashMap<String, Tally>, AppError> {
    let now = now();
    let windows = store::windows(&state.db, now, uptime::rolled_until(now)).await?;
    let rows = store::load_checks(&state.db).await?;
    Ok(rows
        .into_iter()
        .filter(|r| r.check.kind == "app")
        .filter_map(|r| Some((r.check.owner_id, windows.get(&r.check.id)?.day)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    fn req(path: &str, interval: i64, status: Option<i64>, timeout: i64) -> UpdateCheckRequest {
        UpdateCheckRequest {
            enabled: true,
            path: path.into(),
            interval_secs: interval,
            expected_status: status,
            timeout_secs: timeout,
        }
    }

    #[test]
    fn settings_are_validated() {
        assert!(validate(&req("/health", 60, None, 10)).is_ok());
        assert!(validate(&req("/", 3600, Some(204), 30)).is_ok());
        assert!(validate(&req("health", 60, None, 10)).is_err());
        assert!(validate(&req("/a b", 60, None, 10)).is_err());
        assert!(validate(&req("/", 90, None, 10)).is_err());
        assert!(validate(&req("/", 7200, None, 10)).is_err());
        assert!(validate(&req("/", 60, Some(42), 10)).is_err());
        assert!(validate(&req("/", 60, None, 0)).is_err());
        assert_eq!(
            validate(&req("  /ok ", 120, None, 5)).map(|s| s.path),
            Ok("/ok".to_string())
        );
    }

    async fn body(resp: impl IntoResponse) -> serde_json::Value {
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    #[tokio::test]
    async fn app_checks_are_described_updated_and_listed() {
        let state = crate::uptime::tests::test_state().await;
        sqlx::query(
            "INSERT INTO apps (id, name, domain, created_at, updated_at, status)
             VALUES ('app-1', 'web', 'web.example.com', '2026-01-01', '2026-01-01', 'running'),
                    ('app-2', 'worker', NULL, '2026-01-01', '2026-01-01', 'running')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("apps: {e}"));
        let admin = || Principal::test_user(Role::Admin);

        let v = body(
            get_app_check(
                RateLimit,
                Authz::unchecked(admin()),
                State(state.clone()),
                Path("web".into()),
            )
            .await
            .unwrap_or_else(|e| panic!("get: {}", e.0)),
        )
        .await;
        let d = &v["data"];
        assert_eq!(d["url"], "https://web.example.com/");
        assert_eq!(d["status"], "pending");
        assert_eq!(d["check"]["interval_secs"], 60);
        assert_eq!(d["recent"].as_array().map(Vec::len), Some(48));
        assert_eq!(d["daily"].as_array().map(Vec::len), Some(90));

        let v = body(
            get_app_check(
                RateLimit,
                Authz::unchecked(admin()),
                State(state.clone()),
                Path("worker".into()),
            )
            .await
            .unwrap_or_else(|e| panic!("get: {}", e.0)),
        )
        .await;
        assert!(v["data"]["check"].is_null());
        assert!(v["data"]["paused_reason"].as_str().is_some());

        let v = body(
            update_app_check(
                RateLimit,
                Authz::unchecked(admin()),
                State(state.clone()),
                Path("app-1".into()),
                Json(UpdateCheckRequest {
                    enabled: false,
                    ..req("/health", 300, Some(204), 5)
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("put: {}", e.0)),
        )
        .await;
        assert_eq!(v["data"]["status"], "paused");
        assert_eq!(v["data"]["check"]["expected_status"], 204);
        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'uptime_check_update'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_default();
        assert_eq!(audited, 1);

        let bad = update_app_check(
            RateLimit,
            Authz::unchecked(admin()),
            State(state.clone()),
            Path("worker".into()),
            Json(req("/", 60, None, 10)),
        )
        .await;
        assert!(bad.is_err(), "no domain, no check");

        let v = body(
            list_checks(RateLimit, Authz::unchecked(admin()), State(state.clone()))
                .await
                .unwrap_or_else(|e| panic!("list: {}", e.0)),
        )
        .await;
        let names: Vec<&str> = v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|r| r["name"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(names, vec!["Dashboard", "web"]);
        assert_eq!(
            v["data"][0]["status"], "paused",
            "no dashboard domain in tests"
        );

        let now =
            check_dashboard_now(RateLimit, Authz::unchecked(admin()), State(state.clone())).await;
        assert!(now.is_err(), "nothing to probe without a dashboard domain");
    }
}
