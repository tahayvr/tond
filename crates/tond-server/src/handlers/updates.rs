// handlers/updates.rs — System → Updates: Tond updating itself
//
//   GET  /api/system/update           (metrics:read) running version, latest
//                                     release, changelog, the current or last
//                                     run (progress + log tail), rollback offer
//   POST /api/system/update           (settings:write) {version?, skip_snapshot?, force?}
//                                     snapshot, then update in tond-update.service
//   POST /api/system/update/rollback  (settings:write) {skip_snapshot?} back to
//                                     the previous version (within 7 days)
//   POST /api/system/update/check     (settings:write) look for releases now
//                                     (throttled to once a minute)
//   PUT  /api/system/update/settings  (settings:write) {check_enabled}
//
// Starting a run is audited here; its outcome is audited once it finishes
// (updates::audit_finished_run). The dashboard polls GET while a run is going:
// the server restarts in the middle, so there is no stream to keep open.
// `tond update` uses the same endpoints over the root-only socket.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use tond_common::{ApiResponse, TondError};

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Authz, RateLimit, scope},
    error::AppError,
    state::AppState,
    updates::{
        self, releases,
        run::{
            self, Paths, RollbackOption, RunKind, RunState, RunStatus, StartError, StartRequest,
        },
        version::{self, Version},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/system/update", get(get_update).post(start_update))
        .route("/api/system/update/rollback", post(start_rollback))
        .route("/api/system/update/check", post(check_now))
        .route("/api/system/update/settings", put(update_settings))
}

fn ok<T: Serialize>(status: StatusCode, body: T) -> impl IntoResponse {
    (status, Json(ApiResponse::ok(body)))
}

#[derive(Debug, Serialize)]
pub struct ReleaseResponse {
    pub version: String,
    pub tag: String,
    pub name: String,
    pub url: String,
    pub published_at: Option<String>,
    /// Release notes (Markdown).
    pub body: String,
}

impl From<&releases::Release> for ReleaseResponse {
    fn from(r: &releases::Release) -> Self {
        Self {
            version: r.version.clone(),
            tag: r.tag.clone(),
            name: r.name.clone(),
            url: r.url.clone(),
            published_at: r.published_at.clone(),
            body: r.body.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CheckResponse {
    pub enabled: bool,
    pub checked_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    #[serde(flatten)]
    pub status: RunStatus,
    /// `state` as written, except a run whose process vanished is `interrupted`.
    pub effective_state: RunState,
    /// The last lines of the run's output.
    pub log: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateOverview {
    pub current_version: String,
    pub schema_version: Option<i64>,
    /// The newest published release (may equal the current version).
    pub latest: Option<ReleaseResponse>,
    pub update_available: bool,
    /// Releases newer than the running version, newest first.
    pub changelog: Vec<ReleaseResponse>,
    pub check: CheckResponse,
    /// The current or last update/rollback run.
    pub run: Option<RunResponse>,
    /// "Roll back to the previous version", when offered.
    pub rollback: Option<RollbackOption>,
    /// Why the dashboard can't run updates on this server (no systemd, …).
    pub unsupported_reason: Option<String>,
}

pub async fn overview(state: &AppState, paths: &Paths) -> Result<UpdateOverview, AppError> {
    let current = version::current();
    let cached = releases::load(&state.db)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    let newer = releases::newer_than(&cached.releases, &current);
    let now = chrono::Utc::now().timestamp();
    let run = run::read_status(paths).map(|status| {
        let effective_state = run::effective_state(&status, run::pid_alive, now);
        RunResponse {
            log: run::log_tail(paths, run::LOG_TAIL_LINES),
            status,
            effective_state,
        }
    });
    if run
        .as_ref()
        .is_some_and(|r| r.effective_state.is_finished())
    {
        updates::audit_finished_run(&state.db, paths).await;
    }
    let running = run
        .as_ref()
        .is_some_and(|r| r.effective_state == RunState::Running);
    let rollback = if running {
        None
    } else {
        run::rollback_candidate(
            &run::list_backups(&paths.backups_dir),
            &current.to_string(),
            now,
        )
    };
    Ok(UpdateOverview {
        current_version: current.to_string(),
        schema_version: updates::schema_version(&state.db).await,
        latest: cached.releases.first().map(ReleaseResponse::from),
        update_available: !newer.is_empty(),
        changelog: newer.into_iter().map(ReleaseResponse::from).collect(),
        check: CheckResponse {
            enabled: cached.enabled,
            checked_at: cached.checked_at,
            error: cached.error,
        },
        run,
        rollback,
        unsupported_reason: run::unsupported_reason(paths, RunKind::Update),
    })
}

pub async fn get_update(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok(ok(
        StatusCode::OK,
        overview(&state, &Paths::from_env()).await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartUpdateRequest {
    /// The version to install (default: the latest release).
    #[serde(default)]
    pub version: Option<String>,
    /// Don't take a snapshot first (not recommended).
    #[serde(default)]
    pub skip_snapshot: bool,
    /// Reinstall the running version, or install an older one.
    #[serde(default)]
    pub force: bool,
}

/// The version an update request installs, or why it can't.
pub fn resolve_target(
    requested: Option<&str>,
    current: &Version,
    known: &[releases::Release],
    force: bool,
) -> Result<Version, String> {
    let target = match requested.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => Version::parse(v).ok_or_else(|| {
            format!("'{v}' is not a release version (expected something like 1.2.3).")
        })?,
        None => {
            let latest = known
                .first()
                .and_then(|r| Version::parse(&r.version))
                .ok_or_else(|| {
                    "No release is known yet. Check for updates first, or name a version."
                        .to_string()
                })?;
            if &latest <= current && !force {
                return Err(format!(
                    "Tond {current} is the latest release; nothing to update."
                ));
            }
            latest
        }
    };
    if target.is_prerelease() && !force {
        return Err(format!(
            "{target} is a pre-release. Pass force to install it anyway."
        ));
    }
    if &target == current && !force {
        return Err(format!("Tond {current} is already running."));
    }
    if &target < current && !force {
        return Err(format!(
            "{target} is older than the running {current}. Use \"Roll back\" to return to the \
             previous version, or force to install an older release (its database may not \
             understand the newer schema)."
        ));
    }
    Ok(target)
}

fn start_error(e: anyhow::Error) -> AppError {
    match e.downcast_ref::<StartError>() {
        Some(StartError::Busy) => AppError(TondError::Conflict(
            "An update or rollback is already running. Wait for it to finish.".to_string(),
        )),
        Some(StartError::Unsupported(reason)) => AppError(TondError::InvalidInput(reason.clone())),
        None => AppError(TondError::Internal(format!("{e:#}"))),
    }
}

pub async fn start_update(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    body: Option<Json<StartUpdateRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let current = version::current();
    let known = releases::load(&state.db)
        .await
        .map(|c| c.releases)
        .unwrap_or_default();
    let target = resolve_target(req.version.as_deref(), &current, &known, req.force)
        .map_err(|m| AppError(TondError::InvalidInput(m)))?;
    let actor = auth.audit_actor();
    let paths = Paths::from_env();
    let status = run::start(
        &state,
        &paths,
        StartRequest {
            kind: RunKind::Update,
            to_version: target.to_string(),
            backup: None,
            actor: actor.label.clone(),
            skip_snapshot: req.skip_snapshot,
        },
    )
    .await
    .map_err(start_error)?;
    record(
        &state.db,
        &actor,
        AuditEntry::new("tond_update_start")
            .resource("system", "tond", "Tond")
            .detail(format!(
                "update {current} → {target}{}",
                if req.skip_snapshot {
                    " (without a snapshot)"
                } else {
                    ""
                }
            )),
    )
    .await;
    Ok(ok(StatusCode::ACCEPTED, status))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRollbackRequest {
    #[serde(default)]
    pub skip_snapshot: bool,
}

pub async fn start_rollback(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    body: Option<Json<StartRollbackRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let paths = Paths::from_env();
    let current = version::current().to_string();
    let option = run::rollback_candidate(
        &run::list_backups(&paths.backups_dir),
        &current,
        chrono::Utc::now().timestamp(),
    )
    .ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "There is no previous version to roll back to: rollback is offered for 7 days \
             after an update."
                .to_string(),
        ))
    })?;
    let actor = auth.audit_actor();
    let status = run::start(
        &state,
        &paths,
        StartRequest {
            kind: RunKind::Rollback,
            to_version: option.to_version.clone(),
            backup: Some(option.backup.clone()),
            actor: actor.label.clone(),
            skip_snapshot: req.skip_snapshot,
        },
    )
    .await
    .map_err(start_error)?;
    record(
        &state.db,
        &actor,
        AuditEntry::new("tond_rollback_start")
            .resource("system", "tond", "Tond")
            .detail(format!(
                "rollback {current} → {}{}",
                option.to_version,
                if option.restores_database {
                    " (restores the database from before the update)"
                } else {
                    ""
                }
            )),
    )
    .await;
    Ok(ok(StatusCode::ACCEPTED, status))
}

pub async fn check_now(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let cached = releases::load(&state.db)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    let now = chrono::Utc::now().timestamp();
    let recent = cached
        .checked_at
        .is_some_and(|t| now - t < releases::MANUAL_THROTTLE_SECS && t <= now);
    if !recent {
        releases::check_now(&state.db)
            .await
            .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    }
    Ok(ok(
        StatusCode::OK,
        overview(&state, &Paths::from_env()).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettingsRequest {
    pub check_enabled: bool,
}

pub async fn update_settings(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, AppError> {
    releases::set_enabled(&state.db, req.check_enabled)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("update_check_settings")
            .resource("settings", "update_check", "Update check")
            .detail(if req.check_enabled {
                "daily update check on"
            } else {
                "daily update check off"
            }),
    )
    .await;
    Ok(ok(
        StatusCode::OK,
        overview(&state, &Paths::from_env()).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    fn release(v: &str) -> releases::Release {
        releases::Release {
            version: v.into(),
            tag: format!("v{v}"),
            name: format!("Tond {v}"),
            body: String::new(),
            url: String::new(),
            published_at: None,
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn the_default_target_is_the_latest_release() {
        let known = [release("0.5.0"), release("0.4.0")];
        assert_eq!(
            resolve_target(None, &v("0.4.0"), &known, false),
            Ok(v("0.5.0"))
        );
        let err = resolve_target(None, &v("0.5.0"), &known, false).unwrap_err();
        assert!(err.contains("latest release"), "{err}");
        let err = resolve_target(None, &v("0.5.0"), &[], false).unwrap_err();
        assert!(err.contains("No release is known"), "{err}");
    }

    #[test]
    fn a_named_target_must_be_a_newer_stable_version_unless_forced() {
        let cur = v("0.4.0");
        assert_eq!(
            resolve_target(Some("v0.6.1"), &cur, &[], false),
            Ok(v("0.6.1"))
        );
        assert!(resolve_target(Some("latest"), &cur, &[], false).is_err());
        assert!(resolve_target(Some("0.4.0"), &cur, &[], false).is_err());
        assert!(
            resolve_target(Some("0.3.0"), &cur, &[], false)
                .unwrap_err()
                .contains("Roll back")
        );
        assert!(resolve_target(Some("0.5.0-rc.1"), &cur, &[], false).is_err());
        // Forced: reinstall, downgrade, pre-release.
        assert_eq!(
            resolve_target(Some("0.4.0"), &cur, &[], true),
            Ok(v("0.4.0"))
        );
        assert_eq!(
            resolve_target(Some("0.3.0"), &cur, &[], true),
            Ok(v("0.3.0"))
        );
        assert_eq!(
            resolve_target(Some("0.5.0-rc.1"), &cur, &[], true),
            Ok(v("0.5.0-rc.1"))
        );
    }

    #[tokio::test]
    async fn the_overview_reports_version_releases_and_no_run() {
        let state = AppState::for_tests().await;
        let json = format!(
            r#"[{{"tag_name":"v99.0.0","name":"Tond 99","body":"notes","html_url":"u","published_at":null,"draft":false,"prerelease":false}},
               {{"tag_name":"v{}","name":"","body":"","html_url":"u","published_at":null,"draft":false,"prerelease":false}}]"#,
            env!("CARGO_PKG_VERSION")
        );
        let parsed = releases::parse_github(&json).unwrap_or_else(|e| panic!("{e}"));
        releases::store(&state.db, 1, &Ok(parsed))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let paths = Paths {
            update_dir: dir.path().join("update"),
            backups_dir: dir.path().join("backups"),
            bin_dir: dir.path().join("bin"),
        };
        let o = overview(&state, &paths)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(o.current_version, env!("CARGO_PKG_VERSION"));
        assert!(o.update_available);
        assert_eq!(o.changelog.len(), 1);
        assert_eq!(o.changelog[0].body, "notes");
        assert_eq!(o.latest.map(|l| l.version).as_deref(), Some("99.0.0"));
        assert!(o.run.is_none());
        assert!(o.rollback.is_none());
        assert!(o.schema_version.is_some());
        // No tond binary in bin_dir (and no systemd on test machines).
        assert!(o.unsupported_reason.is_some());
    }

    #[tokio::test]
    async fn only_settings_writers_may_start_updates() {
        let viewer = Principal::test_user(Role::Viewer);
        assert!(viewer.require(tond_common::Scope::SettingsWrite).is_err());
        assert!(viewer.require(tond_common::Scope::MetricsRead).is_ok());
    }
}
