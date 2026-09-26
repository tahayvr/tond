// handlers/app_events.rs — one timeline of what happened to an app
//
//   list_app_events()  GET /api/apps/{id}/events?limit=50&before=<RFC 3339>
//
// Built on request from the tables that already hold each fact, newest first:
//
//   deployments     deploy started / succeeded / failed / superseded
//   metric_events   restarts, crashes and OOM kills (metrics collector)
//   certificates    the app domain's last issuance and last failure
//   audit_log       domain, resource-limit and other configuration changes,
//                   start / stop / restart by a person or token — only for
//                   callers who may read the audit log (events:read)
//
// Each source is read through an index with the same `before` bound and
// `limit`, then merged; `next_before` pages further back.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;
use tond_common::*;

use super::fetch_app;
use crate::auth::{Authz, RateLimit, scope};
use crate::error::AppError;
use crate::state::AppState;

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;

/// Audit actions that belong on an app's timeline, and how to show them.
const AUDIT_ACTIONS: &[(&str, AppEventKind, &str)] = &[
    (
        "domain_update",
        AppEventKind::DomainChanged,
        "Domain changed",
    ),
    (
        "resources_update",
        AppEventKind::ResourcesChanged,
        "Resource limits changed",
    ),
    ("port_update", AppEventKind::ConfigChanged, "Port changed"),
    (
        "health_update",
        AppEventKind::ConfigChanged,
        "Health check changed",
    ),
    (
        "readiness_update",
        AppEventKind::ConfigChanged,
        "Readiness check changed",
    ),
    (
        "env_update",
        AppEventKind::ConfigChanged,
        "Environment variables changed",
    ),
    ("volume_add", AppEventKind::ConfigChanged, "Volume added"),
    (
        "volume_remove",
        AppEventKind::ConfigChanged,
        "Volume removed",
    ),
    (
        "source_update",
        AppEventKind::ConfigChanged,
        "Source changed",
    ),
    (
        "auto_deploy_update",
        AppEventKind::ConfigChanged,
        "Auto-deploy changed",
    ),
    (
        "drain_timeout_update",
        AppEventKind::ConfigChanged,
        "Drain timeout changed",
    ),
    ("start", AppEventKind::Control, "Started"),
    ("stop", AppEventKind::Control, "Stopped"),
    ("restart", AppEventKind::Control, "Restarted"),
];

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub limit: Option<i64>,
    pub before: Option<String>,
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn event(
    at: DateTime<Utc>,
    kind: AppEventKind,
    title: impl Into<String>,
    detail: Option<String>,
) -> (DateTime<Utc>, AppEvent) {
    (
        at,
        AppEvent {
            at: fmt_time(at),
            kind,
            severity: kind.severity().to_string(),
            title: title.into(),
            detail: detail.filter(|d| !d.trim().is_empty()),
            actor: None,
            deployment_id: None,
        },
    )
}

/// "42 s", "3 min 5 s", "1 h 2 min".
pub fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        return format!("{secs} s");
    }
    let (m, s) = (secs / 60, secs % 60);
    if m < 60 {
        return if s == 0 {
            format!("{m} min")
        } else {
            format!("{m} min {s} s")
        };
    }
    format!("{} h {} min", m / 60, m % 60)
}

#[derive(sqlx::FromRow)]
struct DeployRow {
    id: String,
    status: String,
    kind: String,
    trigger_detail: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    commit_sha: Option<String>,
    commit_message: Option<String>,
    error: Option<String>,
}

fn deploy_event(row: DeployRow) -> Option<(DateTime<Utc>, AppEvent)> {
    let started = parse_time(&row.started_at)?;
    let finished = row.finished_at.as_deref().and_then(parse_time);
    let what = if row.kind == "rollback" {
        "Rollback"
    } else {
        "Deploy"
    };
    let commit = match (&row.commit_sha, &row.commit_message) {
        (Some(sha), Some(msg)) => Some(format!(
            "{} {}",
            sha.chars().take(7).collect::<String>(),
            msg.lines().next().unwrap_or_default()
        )),
        (Some(sha), None) => Some(sha.chars().take(7).collect()),
        _ => None,
    };
    let (at, kind, title, detail) = match (row.status.as_str(), finished) {
        ("succeeded", Some(end)) => (
            end,
            AppEventKind::DeploySucceeded,
            format!(
                "{what} succeeded in {}",
                format_duration((end - started).num_seconds())
            ),
            commit,
        ),
        ("failed", Some(end)) => (
            end,
            AppEventKind::DeployFailed,
            format!("{what} failed"),
            row.error.clone().or(commit),
        ),
        ("superseded" | "cancelled", end) => (
            end.unwrap_or(started),
            AppEventKind::DeployCancelled,
            if row.status == "superseded" {
                format!("{what} replaced by a newer one")
            } else {
                format!("{what} cancelled")
            },
            commit,
        ),
        _ => (
            started,
            AppEventKind::DeployStarted,
            format!("{what} started"),
            commit,
        ),
    };
    let (at, mut ev) = event(at, kind, title, detail);
    ev.actor = row.trigger_detail;
    ev.deployment_id = Some(row.id);
    Some((at, ev))
}

/// The app's events before `before`, newest first, at most `limit`.
pub async fn app_events(
    db: &SqlitePool,
    app: &App,
    include_audit: bool,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<AppEventsResponse, sqlx::Error> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut all: Vec<(DateTime<Utc>, AppEvent)> = Vec::new();
    let mut some_source_full = false;

    // Deployments (created_at / started_at are chrono's to_rfc3339()).
    let deploys = sqlx::query_as::<_, DeployRow>(
        "SELECT id, status, kind, trigger_detail, started_at, finished_at,
                commit_sha, commit_message, error
         FROM deployments WHERE app_id = ? AND created_at < ?
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&app.id)
    .bind(before.to_rfc3339())
    .bind(limit)
    .fetch_all(db)
    .await?;
    some_source_full |= i64::try_from(deploys.len()).unwrap_or(0) >= limit;
    all.extend(deploys.into_iter().filter_map(deploy_event));

    // Restarts, crashes, OOM kills.
    let happenings: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT e.ts, e.kind, e.detail FROM metric_events e
         JOIN metric_targets t ON t.id = e.target
         WHERE t.kind = 'app' AND t.owner_id = ? AND e.ts < ?
         ORDER BY e.ts DESC LIMIT ?",
    )
    .bind(&app.id)
    .bind(before.timestamp())
    .bind(limit)
    .fetch_all(db)
    .await?;
    some_source_full |= i64::try_from(happenings.len()).unwrap_or(0) >= limit;
    for (ts, kind, detail) in happenings {
        let Some(at) = DateTime::<Utc>::from_timestamp(ts, 0) else {
            continue;
        };
        let (kind, title) = match kind.as_str() {
            "oom" => (AppEventKind::Oom, "Killed: out of memory"),
            "crash" => (AppEventKind::Crash, "Container crashed"),
            _ => (AppEventKind::Restart, "Container restarted"),
        };
        all.push(event(at, kind, title, detail));
    }

    // The domain's certificate: last issuance and last failure.
    if let Some(domain) = app.domain.as_deref().filter(|d| !d.is_empty()) {
        let cert: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT last_renewed_at, last_attempt_at, last_error FROM certificates WHERE domain = ?",
        )
        .bind(domain)
        .fetch_optional(db)
        .await?;
        if let Some((renewed, attempted, error)) = cert {
            if let Some(at) = renewed.as_deref().and_then(parse_time) {
                all.push(event(
                    at,
                    AppEventKind::CertIssued,
                    format!("HTTPS certificate issued for {domain}"),
                    None,
                ));
            }
            if let (Some(at), Some(error)) = (attempted.as_deref().and_then(parse_time), error) {
                all.push(event(
                    at,
                    AppEventKind::CertFailed,
                    format!("Certificate request failed for {domain}"),
                    Some(error),
                ));
            }
        }
    }

    // Changes made by people and tokens (audit timestamps are millis + Z).
    if include_audit {
        let actions: Vec<&str> = AUDIT_ACTIONS.iter().map(|(a, _, _)| *a).collect();
        let placeholders = vec!["?"; actions.len()].join(", ");
        let sql = format!(
            "SELECT created_at, action, actor_label, detail FROM audit_log
             WHERE resource_type = 'app' AND resource_id = ? AND outcome = 'success'
               AND action IN ({placeholders}) AND created_at < ?
             ORDER BY seq DESC LIMIT ?"
        );
        let mut query = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            sqlx::AssertSqlSafe(sql.as_str()),
        )
        .bind(&app.id);
        for action in &actions {
            query = query.bind(*action);
        }
        let rows = query
            .bind(fmt_time(before))
            .bind(limit)
            .fetch_all(db)
            .await?;
        some_source_full |= i64::try_from(rows.len()).unwrap_or(0) >= limit;
        for (created_at, action, actor, detail) in rows {
            let (Some(at), Some((_, kind, title))) = (
                parse_time(&created_at),
                AUDIT_ACTIONS.iter().find(|(a, _, _)| *a == action),
            ) else {
                continue;
            };
            let (at, mut ev) = event(at, *kind, *title, detail);
            ev.actor = Some(actor);
            all.push((at, ev));
        }
    }

    all.retain(|(at, _)| *at < before);
    all.sort_by(|a, b| b.0.cmp(&a.0));
    let more = some_source_full || i64::try_from(all.len()).unwrap_or(0) > limit;
    all.truncate(usize::try_from(limit).unwrap_or(50));
    let next_before = if more {
        all.last().map(|(at, _)| fmt_time(*at))
    } else {
        None
    };
    Ok(AppEventsResponse {
        events: all.into_iter().map(|(_, e)| e).collect(),
        next_before,
    })
}

pub async fn list_app_events(
    _rate: RateLimit,
    auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let before = match q.before.as_deref() {
        Some(s) => parse_time(s).ok_or_else(|| {
            TondError::InvalidInput("before must be an RFC 3339 time".to_string())
        })?,
        None => Utc::now() + chrono::Duration::minutes(1),
    };
    let response = app_events(
        &state.db,
        &app,
        auth.has(Scope::EventsRead),
        before,
        q.limit.unwrap_or(DEFAULT_LIMIT),
    )
    .await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(response))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_log::{AuditActor, write_audit_log};

    async fn seed(db: &SqlitePool) -> App {
        let now = "2026-09-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO apps (id, name, domain, created_at, updated_at)
             VALUES ('a1', 'web', 'web.example.com', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("app: {e}"));
        for (id, status, created, finished, sha) in [
            (
                "d1",
                "succeeded",
                "2026-09-20T10:00:00+00:00",
                Some("2026-09-20T10:01:12+00:00"),
                Some("abcdef1234"),
            ),
            (
                "d2",
                "failed",
                "2026-09-21T10:00:00+00:00",
                Some("2026-09-21T10:00:30+00:00"),
                None,
            ),
            ("d3", "running", "2026-09-22T10:00:00+00:00", None, None),
        ] {
            sqlx::query(
                "INSERT INTO deployments (id, app_id, status, started_at, finished_at, created_at,
                                          commit_sha, commit_message, error, trigger_detail)
                 VALUES (?, 'a1', ?, ?, ?, ?, ?, 'Fix login\nlong body', 'build failed: exit 1', 'dev@example.com')",
            )
            .bind(id)
            .bind(status)
            .bind(created)
            .bind(finished)
            .bind(created)
            .bind(sha)
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("deploy: {e}"));
        }
        sqlx::query("INSERT INTO metric_targets (id, kind, owner_id) VALUES (7, 'app', 'a1')")
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("target: {e}"));
        let oom_ts = parse_time("2026-09-21T12:00:00Z").map_or(0, |t| t.timestamp());
        sqlx::query(
            "INSERT INTO metric_events (target, ts, kind, detail) VALUES (7, ?, 'oom', 'exit code 137')",
        )
        .bind(oom_ts)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("event: {e}"));
        sqlx::query(
            "INSERT INTO certificates (id, domain, cert_path, key_path, issued_at, expires_at, status, last_renewed_at)
             VALUES ('c1', 'web.example.com', '/c', '/k', 'x', 'y', 'active', '2026-09-20T10:02:00+00:00')",
        )
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("cert: {e}"));
        write_audit_log(
            db,
            Some("a1"),
            Some("web"),
            "resources_update",
            &AuditActor::user("u1", "admin@example.com", None),
            Some("memory=512MiB"),
        )
        .await;
        sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = 'a1'")
            .fetch_one(db)
            .await
            .unwrap_or_else(|e| panic!("load app: {e}"))
    }

    #[tokio::test]
    async fn events_merge_every_source_newest_first() {
        let state = AppState::for_tests().await;
        let app = seed(&state.db).await;
        let res = app_events(&state.db, &app, true, Utc::now(), 50)
            .await
            .unwrap_or_else(|e| panic!("events: {e}"));
        let kinds: Vec<AppEventKind> = res.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AppEventKind::ResourcesChanged,
                AppEventKind::DeployStarted,
                AppEventKind::Oom,
                AppEventKind::DeployFailed,
                AppEventKind::CertIssued,
                AppEventKind::DeploySucceeded,
            ]
        );
        let ok = &res.events[5];
        assert_eq!(ok.title, "Deploy succeeded in 1 min 12 s");
        assert_eq!(ok.detail.as_deref(), Some("abcdef1 Fix login"));
        assert_eq!(ok.actor.as_deref(), Some("dev@example.com"));
        assert_eq!(ok.deployment_id.as_deref(), Some("d1"));
        assert_eq!(ok.severity, "success");
        assert_eq!(res.events[2].detail.as_deref(), Some("exit code 137"));
        assert_eq!(
            res.events[3].detail.as_deref(),
            Some("build failed: exit 1")
        );
        assert_eq!(res.events[0].actor.as_deref(), Some("admin@example.com"));
        assert_eq!(res.next_before, None);

        // Without events:read, audit-derived events are left out.
        let res = app_events(&state.db, &app, false, Utc::now(), 50)
            .await
            .unwrap_or_else(|e| panic!("events: {e}"));
        assert!(
            !res.events
                .iter()
                .any(|e| e.kind == AppEventKind::ResourcesChanged)
        );
    }

    #[tokio::test]
    async fn events_page_backwards() {
        let state = AppState::for_tests().await;
        let app = seed(&state.db).await;
        let first = app_events(&state.db, &app, true, Utc::now(), 2)
            .await
            .unwrap_or_else(|e| panic!("events: {e}"));
        assert_eq!(first.events.len(), 2);
        let before = first
            .next_before
            .as_deref()
            .and_then(parse_time)
            .unwrap_or_else(|| panic!("more pages expected"));
        let second = app_events(&state.db, &app, true, before, 2)
            .await
            .unwrap_or_else(|e| panic!("events: {e}"));
        assert_eq!(
            second.events.first().map(|e| e.kind),
            Some(AppEventKind::Oom)
        );
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(42), "42 s");
        assert_eq!(format_duration(72), "1 min 12 s");
        assert_eq!(format_duration(120), "2 min");
        assert_eq!(format_duration(3720), "1 h 2 min");
    }
}
