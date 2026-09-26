// deploy/progress.rs — structured deployment progress
//
// A running deployment registers itself here (`begin`). From then on:
//   - every deploy log line written for the app (deploy::log_deploy, which
//     is also called from the build helpers and their stdout/stderr readers)
//     is attributed to the deployment (`deploy_logs.deployment_id`) and kept
//     in a small ring buffer so a failure can quote the lines that show it;
//   - phase transitions (`start`, `succeed`, `skip`) update the in-memory
//     phase list, persist it to `deployments.phases` and publish a
//     `DeployEvent::Deployment` on the log bus, so live deploy views re-read
//     the row instead of polling;
//   - `finish_ok` / `finish_failed` write the result (status, finished_at,
//     classified failure) and unregister.
//
// The registry is keyed by app id: an app runs one deployment at a time.
// Preview builds (trigger "preview") never touch it.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

use tond_common::{App, DeployFailure, DeployPhase, PhaseRecord, PhaseStatus, Readiness};

use crate::log_bus;
use crate::state::AppState;

/// Log lines kept per deployment for failure excerpts.
const RECENT_LINES: usize = 60;

struct Active {
    deployment_id: String,
    phases: Vec<PhaseRecord>,
    recent: VecDeque<String>,
}

static ACTIVE: LazyLock<Mutex<HashMap<String, Active>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn with_active<R>(app_id: &str, f: impl FnOnce(&mut Active) -> R) -> Option<R> {
    let mut map = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    map.get_mut(app_id).map(f)
}

/// Label of the health step for an app's readiness mode.
pub fn health_label(
    readiness: Readiness,
    health_path: Option<&str>,
    ready_after_secs: i64,
) -> String {
    match readiness {
        Readiness::Http if health_path.is_some() => "Health check".to_string(),
        Readiness::Http => "Wait for the app to listen".to_string(),
        Readiness::Tcp => "Wait for the port to accept connections".to_string(),
        Readiness::None => format!("Stay up for {ready_after_secs}s"),
    }
}

/// The phase plan for an app: [`plan_for`] with the health and release steps
/// named after its readiness mode (workers have no traffic to switch).
pub fn plan_for_app(plan_source: &str, app: &App) -> Vec<PhaseRecord> {
    if app.source_type == "compose" {
        return compose_plan(plan_source == "rollback", app.source_url.is_some());
    }
    let readiness = app.readiness();
    let mut phases = plan_for(plan_source, app.health_check_path.as_deref());
    for p in phases.iter_mut() {
        if p.status == PhaseStatus::Skipped {
            continue;
        }
        match p.phase {
            DeployPhase::Health => {
                p.label = Some(health_label(
                    readiness,
                    app.health_check_path.as_deref(),
                    app.ready_after_secs,
                ))
            }
            DeployPhase::Release if !readiness.serves_traffic() => {
                p.label = Some("Replace the old container".to_string())
            }
            _ => {}
        }
    }
    phases
}

/// The phase plan for a source type (`rollback` for image rollbacks).
pub fn plan_for(source_type: &str, health_path: Option<&str>) -> Vec<PhaseRecord> {
    use DeployPhase::*;
    let health_label = if health_path.is_some() {
        "Health check"
    } else {
        "Wait for the app to listen"
    };
    let steps: [(DeployPhase, Option<&str>); 5] = match source_type {
        "git" => [
            (Clone, Some("Clone repository")),
            (Build, Some("Build image")),
            (Start, Some("Start container")),
            (Health, Some(health_label)),
            (Release, Some("Switch traffic")),
        ],
        "image" => [
            (Clone, Some("Pull image")),
            (Build, None),
            (Start, Some("Start container")),
            (Health, Some(health_label)),
            (Release, Some("Switch traffic")),
        ],
        "tarball" => [
            (Clone, None),
            (Build, Some("Build uploaded source")),
            (Start, Some("Start container")),
            (Health, Some(health_label)),
            (Release, Some("Switch traffic")),
        ],
        "dockerfile" => [
            (Clone, None),
            (Build, Some("Build image")),
            (Start, Some("Start container")),
            (Health, Some(health_label)),
            (Release, Some("Switch traffic")),
        ],
        "compose" => [
            (Clone, None),
            (Build, Some("Build and pull service images")),
            (Start, Some("Start services")),
            (Health, Some("Wait for services to be ready")),
            (Release, Some("Switch traffic")),
        ],
        // "rollback" and anything unknown: re-run an existing image.
        _ => [
            (Clone, None),
            (Build, None),
            (Start, Some("Start container")),
            (Health, Some(health_label)),
            (Release, Some("Switch traffic")),
        ],
    };
    steps
        .into_iter()
        .map(|(phase, label)| PhaseRecord {
            phase,
            status: if label.is_some() {
                PhaseStatus::Pending
            } else {
                PhaseStatus::Skipped
            },
            label: label.map(str::to_string),
            started_at: None,
            finished_at: None,
        })
        .collect()
}

/// The phase plan of a compose deployment: clone only for a repository,
/// no build for a rollback (the pinned images exist).
pub fn compose_plan(rollback: bool, from_git: bool) -> Vec<PhaseRecord> {
    let mut phases = plan_for("compose", None);
    for p in phases.iter_mut() {
        match p.phase {
            DeployPhase::Clone if from_git && !rollback => {
                p.status = PhaseStatus::Pending;
                p.label = Some("Clone repository".to_string());
            }
            DeployPhase::Build if rollback => {
                p.label = Some("Reuse the deployment's images".to_string());
            }
            _ => {}
        }
    }
    phases
}

async fn persist(state: &AppState, app_id: &str, deployment_id: &str, phases: &[PhaseRecord]) {
    let json = serde_json::to_string(phases).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query("UPDATE deployments SET phases = ? WHERE id = ?")
        .bind(json)
        .bind(deployment_id)
        .execute(&state.db)
        .await;
    log_bus::publish_deployment(app_id, deployment_id);
}

/// Register a deployment as the app's active one and store its phase plan.
pub async fn begin(state: &AppState, app_id: &str, deployment_id: &str, phases: Vec<PhaseRecord>) {
    {
        let mut map = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(
            app_id.to_string(),
            Active {
                deployment_id: deployment_id.to_string(),
                phases: phases.clone(),
                recent: VecDeque::with_capacity(RECENT_LINES),
            },
        );
    }
    persist(state, app_id, deployment_id, &phases).await;
}

/// The app's active deployment, if one is registered.
pub fn active_deployment(app_id: &str) -> Option<String> {
    with_active(app_id, |a| a.deployment_id.clone())
}

/// Remember a log line for failure excerpts.
pub fn record_line(app_id: &str, line: &str) {
    with_active(app_id, |a| {
        if a.recent.len() == RECENT_LINES {
            a.recent.pop_front();
        }
        a.recent.push_back(line.to_string());
    });
}

/// The most recent log lines of the app's active deployment.
pub fn recent_lines(app_id: &str) -> Vec<String> {
    with_active(app_id, |a| a.recent.iter().cloned().collect()).unwrap_or_default()
}

/// The phase that is running now, if any.
pub fn current_phase(app_id: &str) -> Option<DeployPhase> {
    with_active(app_id, |a| {
        a.phases
            .iter()
            .find(|p| p.status == PhaseStatus::Running)
            .map(|p| p.phase)
    })
    .flatten()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Apply `f` to the phase list and persist the result.
async fn update(state: &AppState, app_id: &str, f: impl FnOnce(&mut Vec<PhaseRecord>)) {
    let snapshot = with_active(app_id, |a| {
        f(&mut a.phases);
        (a.deployment_id.clone(), a.phases.clone())
    });
    if let Some((deployment_id, phases)) = snapshot {
        persist(state, app_id, &deployment_id, &phases).await;
    }
}

/// Mark `phase` running. Any phase still running before it is marked succeeded.
pub async fn start(state: &AppState, app_id: &str, phase: DeployPhase) {
    let ts = now();
    update(state, app_id, |phases| mark_start(phases, phase, &ts)).await;
}

/// Change a phase's label (e.g. when the health check turns out to be a port probe).
pub async fn relabel(state: &AppState, app_id: &str, phase: DeployPhase, label: &str) {
    update(state, app_id, |phases| {
        if let Some(p) = phases.iter_mut().find(|p| p.phase == phase) {
            p.label = Some(label.to_string());
        }
    })
    .await;
}

fn mark_start(phases: &mut [PhaseRecord], phase: DeployPhase, ts: &str) {
    for p in phases.iter_mut() {
        if p.phase == phase {
            p.status = PhaseStatus::Running;
            p.started_at = Some(ts.to_string());
            p.finished_at = None;
        } else if p.status == PhaseStatus::Running {
            p.status = PhaseStatus::Succeeded;
            p.finished_at = Some(ts.to_string());
        }
    }
}

/// Final phase states for a successful deployment: running phases succeed,
/// phases that never started are skipped.
fn close_ok(phases: &mut [PhaseRecord], ts: &str) {
    for p in phases.iter_mut() {
        match p.status {
            PhaseStatus::Running => {
                p.status = PhaseStatus::Succeeded;
                p.finished_at = Some(ts.to_string());
            }
            PhaseStatus::Pending => p.status = PhaseStatus::Skipped,
            _ => {}
        }
    }
}

/// Final phase states for a failed deployment: the running phase (or the
/// failure's phase) fails; phases after it stay pending.
fn close_failed(phases: &mut [PhaseRecord], failed: Option<DeployPhase>, ts: &str) {
    let target = failed.or_else(|| {
        phases
            .iter()
            .find(|p| p.status == PhaseStatus::Running)
            .map(|p| p.phase)
    });
    for p in phases.iter_mut() {
        if Some(p.phase) == target {
            if p.started_at.is_none() {
                p.started_at = Some(ts.to_string());
            }
            p.status = PhaseStatus::Failed;
            p.finished_at = Some(ts.to_string());
        } else if p.status == PhaseStatus::Running {
            p.status = PhaseStatus::Succeeded;
            p.finished_at = Some(ts.to_string());
        }
    }
}

fn unregister(app_id: &str, deployment_id: &str) -> Option<Vec<PhaseRecord>> {
    let mut map = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if map.get(app_id).map(|a| a.deployment_id.as_str()) == Some(deployment_id) {
        map.remove(app_id).map(|a| a.phases)
    } else {
        None
    }
}

async fn read_phases(state: &AppState, deployment_id: &str) -> Vec<PhaseRecord> {
    let json: Option<String> = sqlx::query_scalar("SELECT phases FROM deployments WHERE id = ?")
        .bind(deployment_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    json.and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Record a successful result and unregister.
pub async fn finish_ok(state: &AppState, app_id: &str, deployment_id: &str) {
    let ts = now();
    let mut phases = match unregister(app_id, deployment_id) {
        Some(p) => p,
        None => read_phases(state, deployment_id).await,
    };
    close_ok(&mut phases, &ts);
    let json = serde_json::to_string(&phases).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query(
        "UPDATE deployments SET status = 'succeeded', finished_at = ?, phases = ?, failure = NULL WHERE id = ?",
    )
    .bind(&ts)
    .bind(json)
    .bind(deployment_id)
    .execute(&state.db)
    .await;
    log_bus::publish_deployment(app_id, deployment_id);
}

/// Record a failure (with its classified cause) and unregister.
pub async fn finish_failed(
    state: &AppState,
    app_id: &str,
    deployment_id: &str,
    failure: &DeployFailure,
) {
    let ts = now();
    let mut phases = match unregister(app_id, deployment_id) {
        Some(p) => p,
        None => read_phases(state, deployment_id).await,
    };
    close_failed(&mut phases, failure.phase, &ts);
    let json = serde_json::to_string(&phases).unwrap_or_else(|_| "[]".to_string());
    let failure_json = serde_json::to_string(failure).ok();
    let _ = sqlx::query(
        "UPDATE deployments SET status = 'failed', finished_at = ?, phases = ?, failure = ? WHERE id = ?",
    )
    .bind(&ts)
    .bind(json)
    .bind(failure_json)
    .bind(deployment_id)
    .execute(&state.db)
    .await;
    log_bus::publish_deployment(app_id, deployment_id);
}

/// Store the commit a git deploy built.
pub async fn record_commit(
    state: &AppState,
    app_id: &str,
    deployment_id: &str,
    commit: &super::git::CommitInfo,
) {
    let _ = sqlx::query(
        "UPDATE deployments SET commit_sha = ?, commit_message = ?, commit_author = ? WHERE id = ?",
    )
    .bind(&commit.sha)
    .bind(&commit.message)
    .bind(&commit.author)
    .bind(deployment_id)
    .execute(&state.db)
    .await;
    log_bus::publish_deployment(app_id, deployment_id);
}

/// At startup: deployments still "running" were cut off by a restart. Mark
/// them failed with an explanation, and put apps left "building" back to
/// "running" when they still have a container, else "failed".
pub async fn fail_interrupted(db: &sqlx::SqlitePool) -> sqlx::Result<u64> {
    let failure = DeployFailure {
        phase: None,
        code: "interrupted".to_string(),
        title: "The deployment was interrupted".to_string(),
        hint: "Tond restarted while this deployment was running. Deploy again.".to_string(),
        detail: None,
        excerpt: Vec::new(),
    };
    let failure_json = serde_json::to_string(&failure).unwrap_or_default();
    let now = now();
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM deployments WHERE status IN ('running', 'pending')")
            .fetch_all(db)
            .await?;
    for id in &ids {
        let phases: String = sqlx::query_scalar("SELECT phases FROM deployments WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await?;
        let mut phases: Vec<PhaseRecord> = serde_json::from_str(&phases).unwrap_or_default();
        close_failed(&mut phases, None, &now);
        sqlx::query(
            "UPDATE deployments SET status = 'failed', finished_at = ?, failure = ?, phases = ? WHERE id = ?",
        )
        .bind(&now)
        .bind(&failure_json)
        .bind(serde_json::to_string(&phases).unwrap_or_else(|_| "[]".to_string()))
        .bind(id)
        .execute(db)
        .await?;
    }
    sqlx::query(
        "UPDATE apps SET status = CASE WHEN container_id IS NULL THEN 'failed' ELSE 'running' END,
                updated_at = datetime('now')
         WHERE status = 'building'",
    )
    .execute(db)
    .await?;
    Ok(ids.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn interrupted_deployments_are_failed_at_startup() {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("sqlite: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        let phases = r#"[{"phase":"build","status":"running","started_at":"t"}]"#;
        for (id, status) in [("d1", "running"), ("d2", "succeeded")] {
            sqlx::query(
                "INSERT INTO deployments (id, app_id, status, phases, started_at, created_at) VALUES (?, 'a', ?, ?, 't', 't')",
            )
            .bind(id)
            .bind(status)
            .bind(phases)
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("seed: {e}"));
        }
        assert_eq!(fail_interrupted(&db).await.unwrap_or(0), 1);
        let (status, failure, phases): (String, Option<String>, String) =
            sqlx::query_as("SELECT status, failure, phases FROM deployments WHERE id = 'd1'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(status, "failed");
        assert!(failure.unwrap_or_default().contains("interrupted"));
        assert!(phases.contains("\"failed\""));
        let other: String = sqlx::query_scalar("SELECT status FROM deployments WHERE id = 'd2'")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(other, "succeeded");
    }

    fn statuses(phases: &[PhaseRecord]) -> Vec<(DeployPhase, PhaseStatus)> {
        phases.iter().map(|p| (p.phase, p.status)).collect()
    }

    #[test]
    fn plans_skip_phases_a_source_type_does_not_have() {
        use DeployPhase::*;
        use PhaseStatus::*;
        let git = plan_for("git", Some("/health"));
        assert!(git.iter().all(|p| p.status == Pending));
        assert_eq!(git[3].label.as_deref(), Some("Health check"));

        let image = plan_for("image", None);
        assert_eq!(
            statuses(&image),
            vec![
                (Clone, Pending),
                (Build, Skipped),
                (Start, Pending),
                (Health, Pending),
                (Release, Pending)
            ]
        );
        assert_eq!(image[0].label.as_deref(), Some("Pull image"));
        assert_eq!(
            image[3].label.as_deref(),
            Some("Wait for the app to listen")
        );

        let rollback = plan_for("rollback", None);
        assert_eq!(rollback[0].status, Skipped);
        assert_eq!(rollback[1].status, Skipped);
        let compose = plan_for("compose", None);
        assert_eq!(compose[0].status, Skipped);
        assert_eq!(compose[3].status, Pending);
        let mut app = App::new("s".into(), "compose".into(), None, None, 80);
        assert_eq!(plan_for_app("compose", &app)[0].status, Skipped);
        app.source_url = Some("https://example.com/r.git".into());
        let from_git = plan_for_app("compose", &app);
        assert_eq!(from_git[0].label.as_deref(), Some("Clone repository"));
        let rollback = plan_for_app("rollback", &app);
        assert_eq!(rollback[0].status, Skipped);
        assert_eq!(
            rollback[1].label.as_deref(),
            Some("Reuse the deployment's images")
        );
    }

    #[test]
    fn plans_name_the_health_step_after_the_readiness_mode() {
        let mut app = App::new("w".into(), "image".into(), None, None, 80);
        assert_eq!(
            plan_for_app("image", &app)[3].label.as_deref(),
            Some("Wait for the app to listen")
        );
        app.readiness = "tcp".into();
        assert_eq!(
            plan_for_app("image", &app)[3].label.as_deref(),
            Some("Wait for the port to accept connections")
        );
        app.readiness = "none".into();
        app.ready_after_secs = 15;
        let plan = plan_for_app("image", &app);
        assert_eq!(plan[3].label.as_deref(), Some("Stay up for 15s"));
        assert_eq!(plan[4].label.as_deref(), Some("Replace the old container"));
        assert_eq!(plan[1].status, PhaseStatus::Skipped);
    }

    #[test]
    fn starting_a_phase_closes_the_previous_one() {
        use DeployPhase::*;
        use PhaseStatus::*;
        let mut phases = plan_for("git", None);
        mark_start(&mut phases, Clone, "t1");
        mark_start(&mut phases, Build, "t2");
        assert_eq!(phases[0].status, Succeeded);
        assert_eq!(phases[0].finished_at.as_deref(), Some("t2"));
        assert_eq!(phases[1].status, Running);
        assert_eq!(phases[1].started_at.as_deref(), Some("t2"));
    }

    #[test]
    fn closing_marks_the_failed_phase_and_leaves_later_ones_pending() {
        use DeployPhase::*;
        use PhaseStatus::*;
        let mut phases = plan_for("git", None);
        mark_start(&mut phases, Clone, "t1");
        mark_start(&mut phases, Build, "t2");
        close_failed(&mut phases, None, "t3");
        assert_eq!(
            statuses(&phases),
            vec![
                (Clone, Succeeded),
                (Build, Failed),
                (Start, Pending),
                (Health, Pending),
                (Release, Pending)
            ]
        );

        // A failure can name its phase even when nothing was running.
        let mut phases = plan_for("image", None);
        close_failed(&mut phases, Some(Clone), "t1");
        assert_eq!(phases[0].status, Failed);
        assert_eq!(phases[0].started_at.as_deref(), Some("t1"));

        let mut phases = plan_for("image", None);
        mark_start(&mut phases, Release, "t1");
        close_ok(&mut phases, "t2");
        assert_eq!(phases[4].status, Succeeded);
        assert_eq!(
            phases[0].status, Skipped,
            "never-started phases are skipped"
        );
    }

    #[test]
    fn recent_lines_are_bounded_and_scoped_to_the_active_deployment() {
        let app = "progress-test-app";
        {
            let mut map = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            map.insert(
                app.to_string(),
                Active {
                    deployment_id: "dep".into(),
                    phases: plan_for("git", None),
                    recent: VecDeque::new(),
                },
            );
        }
        for i in 0..(RECENT_LINES + 5) {
            record_line(app, &format!("line {i}"));
        }
        let lines = recent_lines(app);
        assert_eq!(lines.len(), RECENT_LINES);
        assert_eq!(lines[0], "line 5");
        assert_eq!(active_deployment(app).as_deref(), Some("dep"));
        assert!(
            unregister(app, "other").is_none(),
            "only the owner unregisters"
        );
        assert!(unregister(app, "dep").is_some());
        assert!(recent_lines(app).is_empty());
        assert!(active_deployment(app).is_none());
    }
}
