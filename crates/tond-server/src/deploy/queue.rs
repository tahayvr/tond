// deploy/queue.rs — the build/deploy queue
//
// Every production deploy of an app (manual, webhook, project deploy, upload,
// rollback) goes through here instead of being spawned directly:
//
//   - One deploy in flight per app. Deploys of the same app never overlap,
//     so they never race on its build directory, container names or port.
//   - Per-app coalescing: at most one deploy waits behind the running one.
//     A newer request supersedes the waiting one (its row becomes
//     'superseded', `superseded_by` the new id), so a burst of pushes builds
//     only the first and the last.
//   - A global build semaphore (TOND_BUILD_CONCURRENCY, default 1) bounds how
//     many image builds run at once across all apps (and previews). Only the
//     build phase holds a slot; health checks and cutover don't.
//   - Builds run with `--cgroup-parent tondbuild.slice` (see build.rs), a
//     systemd slice install.sh caps at MemoryMax=60%, so a runaway build can't
//     push the host into an OOM that takes out running apps or Tond.
//
// Deployment row lifecycle (deploy::create inserts the row as 'queued'):
//   queued -> running -> succeeded | failed      (phases: deploy::progress)
//   queued -> superseded
// A row stays 'queued' while it waits for the app's previous deploy *or* for
// a build slot; `positions()` gives each queued row its place in line.
//
// Shutdown: the runner starts nothing new once shutdown begins; queued rows
// stay 'queued' and are resumed by reconcile.rs at the next start. Runners
// are spawned as tracked work so SIGTERM waits for an in-flight deploy.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Result;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tond_common::App;

use super::core::{DeployKind, DeployRequest};
use crate::runtime::shutdown;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct Job {
    pub deployment_id: String,
    pub app_id: String,
    pub req: DeployRequest,
}

/// Result of `enqueue`, returned by the API as 202 Accepted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Enqueued {
    pub deployment_id: String,
    pub status: &'static str,
    /// 1-based place in line.
    pub queue_position: usize,
    /// The waiting deployment this one replaced, if any.
    pub superseded: Option<String>,
}

#[derive(Default)]
struct AppSlot {
    /// A runner task is working on this app.
    running: bool,
    /// The deployment the runner is executing.
    active: Option<String>,
    /// The single deployment waiting behind it.
    queued: Option<Job>,
}

#[derive(Default)]
struct Inner {
    slots: HashMap<String, AppSlot>,
    /// Queued deployment ids in arrival order (for positions).
    waiting: VecDeque<String>,
}

pub struct BuildQueue {
    inner: Mutex<Inner>,
    builds: Arc<Semaphore>,
    build_slots: u32,
}

/// Default number of concurrent builds.
pub const DEFAULT_BUILD_CONCURRENCY: u32 = 1;

static QUEUE: LazyLock<BuildQueue> = LazyLock::new(|| BuildQueue::new(concurrency_from_env()));

/// Serialises "pick a free host port + start the container" across all
/// deploys: `docker::find_free_port` reads live container ports, so two
/// concurrent deploys (of different apps, now that only builds are
/// serialised) could otherwise pick the same one.
static PORT_ALLOCATION: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn concurrency_from_env() -> u32 {
    std::env::var("TOND_BUILD_CONCURRENCY")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_BUILD_CONCURRENCY)
}

/// The process-wide queue.
pub fn global() -> &'static BuildQueue {
    &QUEUE
}

/// Hold while allocating a host port and starting the container on it.
pub async fn port_allocation() -> tokio::sync::MutexGuard<'static, ()> {
    PORT_ALLOCATION.lock().await
}

/// A build slot, for builds outside the per-app queue (previews).
pub async fn build_slot() -> Option<OwnedSemaphorePermit> {
    global().builds.clone().acquire_owned().await.ok()
}

impl BuildQueue {
    pub fn new(build_slots: u32) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            builds: Arc::new(Semaphore::new(build_slots as usize)),
            build_slots,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn build_slots(&self) -> u32 {
        self.build_slots
    }

    /// Deployments waiting and deployments being executed.
    pub fn counts(&self) -> (u32, u32) {
        let inner = self.lock();
        let active = inner.slots.values().filter(|s| s.active.is_some()).count();
        (inner.waiting.len() as u32, active as u32)
    }

    /// 1-based position of every queued deployment.
    pub fn positions(&self) -> HashMap<String, usize> {
        self.lock()
            .waiting
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i + 1))
            .collect()
    }

    fn position_of(inner: &Inner, id: &str) -> usize {
        inner
            .waiting
            .iter()
            .position(|w| w == id)
            .map_or(0, |i| i + 1)
    }

    /// Put `job` in its app's slot. Returns (position, superseded job, whether
    /// a runner must be spawned).
    fn admit(&self, job: Job) -> (usize, Option<Job>, bool) {
        let mut inner = self.lock();
        let id = job.deployment_id.clone();
        let slot = inner.slots.entry(job.app_id.clone()).or_default();
        let superseded = slot.queued.replace(job);
        let spawn = !slot.running;
        slot.running = true;
        if let Some(old) = &superseded {
            inner.waiting.retain(|w| w != &old.deployment_id);
        }
        inner.waiting.push_back(id.clone());
        (Self::position_of(&inner, &id), superseded, spawn)
    }

    /// Take the app's next job, or retire the runner if there is none (or
    /// shutdown began). Clears the previous active deployment.
    fn next_job(&self, app_id: &str) -> Option<Job> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(app_id)?;
        slot.active = None;
        if shutdown::is_shutting_down() {
            slot.running = false;
            // Leave the queued job (and its 'queued' row) for the next start.
            return None;
        }
        match slot.queued.take() {
            Some(job) => {
                slot.active = Some(job.deployment_id.clone());
                Some(job)
            }
            None => {
                inner.slots.remove(app_id);
                None
            }
        }
    }

    fn leave_waiting(&self, deployment_id: &str) {
        self.lock().waiting.retain(|w| w != deployment_id);
    }
}

/// Per-deployment handle passed to deploy::run: moves the row out of the
/// queue and holds the build slot during the build phase.
pub struct DeployCtx {
    pub deployment_id: String,
    queue: &'static BuildQueue,
    permit: Mutex<Option<OwnedSemaphorePermit>>,
}

impl DeployCtx {
    fn with_queue(deployment_id: String, queue: &'static BuildQueue) -> Self {
        Self {
            deployment_id,
            queue,
            permit: Mutex::new(None),
        }
    }

    fn has_permit(&self) -> bool {
        self.permit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Leave the queue: mark the row 'running'. Idempotent.
    pub async fn begin(&self, state: &AppState) -> Result<()> {
        self.queue.leave_waiting(&self.deployment_id);
        mark_running(state, &self.deployment_id).await
    }

    /// Wait for a build slot, then [`begin`](Self::begin). Idempotent.
    pub async fn begin_build(&self, state: &AppState) -> Result<()> {
        if !self.has_permit() {
            let permit = self.queue.builds.clone().acquire_owned().await?;
            *self.permit.lock().unwrap_or_else(|e| e.into_inner()) = Some(permit);
        }
        self.begin(state).await
    }

    /// Give the build slot back (the image exists, or the deployment ended).
    pub fn release_build(&self) {
        self.permit.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

async fn mark_running(state: &AppState, deployment_id: &str) -> Result<()> {
    // started_at marks when work (not waiting) began.
    let app_id: Option<String> = sqlx::query_scalar(
        "UPDATE deployments SET status = 'running', started_at = ?
         WHERE id = ? AND status = 'queued' RETURNING app_id",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await?;
    // Live deploy views re-read the row.
    if let Some(app_id) = app_id {
        crate::log_bus::publish_deployment(&app_id, deployment_id);
    }
    Ok(())
}

/// Record a final status (unless the row is already final).
pub async fn finish(
    state: &AppState,
    deployment_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE deployments SET status = ?, error = COALESCE(?, error), finished_at = ?
         WHERE id = ? AND status IN ('queued', 'running')",
    )
    .bind(status)
    .bind(error)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(deployment_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Hand a created ('queued') deployment to the app's runner.
pub async fn submit_request(
    state: &AppState,
    app_id: &str,
    deployment_id: String,
    req: DeployRequest,
) -> Enqueued {
    let job = Job {
        deployment_id,
        app_id: app_id.to_string(),
        req,
    };
    submit(state, global(), job).await
}

/// Hand an existing 'queued' row back to the queue (startup resume).
pub async fn resume(state: &AppState, job: Job) -> Enqueued {
    submit(state, global(), job).await
}

async fn submit(state: &AppState, queue: &'static BuildQueue, job: Job) -> Enqueued {
    let deployment_id = job.deployment_id.clone();
    let app_id = job.app_id.clone();
    let (queue_position, superseded, spawn) = queue.admit(job);

    let superseded_id = superseded.as_ref().map(|j| j.deployment_id.clone());
    if let Some(old) = superseded {
        let _ = sqlx::query(
            "UPDATE deployments
             SET status = 'superseded', superseded_by = ?, finished_at = ?,
                 error = 'Replaced by a newer deploy before it started'
             WHERE id = ? AND status = 'queued'",
        )
        .bind(&deployment_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&old.deployment_id)
        .execute(&state.db)
        .await;
        crate::log_bus::publish_deployment(&app_id, &old.deployment_id);
        if let DeployKind::Upload { dir } = &old.req.kind {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        tracing::info!(
            "Deployment {} superseded by {}",
            old.deployment_id,
            deployment_id
        );
    }

    if spawn {
        let state = state.clone();
        shutdown::spawn_tracked(async move { run_app(state, queue, app_id).await });
    }

    Enqueued {
        deployment_id,
        status: "queued",
        queue_position,
        superseded: superseded_id,
    }
}

/// Runner: execute the app's jobs one after another until none is waiting.
async fn run_app(state: AppState, queue: &'static BuildQueue, app_id: String) {
    while let Some(job) = queue.next_job(&app_id) {
        let ctx = DeployCtx::with_queue(job.deployment_id.clone(), queue);
        execute(&state, &ctx, &job).await;
        ctx.release_build();
        queue.leave_waiting(&job.deployment_id);
    }
}

async fn execute(state: &AppState, ctx: &DeployCtx, job: &Job) {
    let app: Option<App> = sqlx::query_as("SELECT * FROM apps WHERE id = ?")
        .bind(&job.app_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let Some(mut app) = app else {
        let _ = finish(
            state,
            &job.deployment_id,
            "failed",
            Some("The app was deleted"),
        )
        .await;
        if let DeployKind::Upload { dir } = &job.req.kind {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        return;
    };
    // deploy::run records phases, the result, the app status and alerts.
    super::run(state, &mut app, &job.deployment_id, &job.req, ctx).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, app: &str) -> Job {
        Job {
            deployment_id: id.to_string(),
            app_id: app.to_string(),
            req: DeployRequest::source("manual", None),
        }
    }

    #[test]
    fn a_newer_request_supersedes_the_waiting_one_but_not_the_running_one() {
        let q = BuildQueue::new(1);
        let (pos, superseded, spawn) = q.admit(job("d1", "app"));
        assert_eq!((pos, superseded.is_none(), spawn), (1, true, true));

        // The runner picks d1 up; it is no longer waiting once it builds.
        assert_eq!(
            q.next_job("app").map(|j| j.deployment_id),
            Some("d1".into())
        );
        q.leave_waiting("d1");

        let (pos, superseded, spawn) = q.admit(job("d2", "app"));
        assert_eq!(
            (pos, superseded.is_none(), spawn),
            (1, true, false),
            "runner already active"
        );
        let (pos, superseded, _) = q.admit(job("d3", "app"));
        assert_eq!(pos, 1, "d2 left the line");
        assert_eq!(superseded.map(|j| j.deployment_id), Some("d2".into()));
        assert_eq!(q.positions().get("d3"), Some(&1));
        assert_eq!(q.positions().get("d2"), None);

        // d1 finishes -> d3 runs next, then the runner retires.
        assert_eq!(
            q.next_job("app").map(|j| j.deployment_id),
            Some("d3".into())
        );
        q.leave_waiting("d3");
        assert!(q.next_job("app").is_none());
        let (_, _, spawn) = q.admit(job("d4", "app"));
        assert!(spawn, "a retired runner is spawned again");
    }

    #[test]
    fn positions_follow_arrival_order_across_apps() {
        let q = BuildQueue::new(1);
        q.admit(job("a1", "a"));
        q.admit(job("b1", "b"));
        q.admit(job("c1", "c"));
        let p = q.positions();
        assert_eq!((p["a1"], p["b1"], p["c1"]), (1, 2, 3));
        q.leave_waiting("a1");
        assert_eq!(q.positions()["c1"], 2);
        assert_eq!(q.counts().0, 2);
    }

    async fn test_state() -> AppState {
        use bollard::{API_DEFAULT_VERSION, Docker};
        use tokio::sync::RwLock;
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 1, API_DEFAULT_VERSION)
                .unwrap_or_else(|e| panic!("docker: {e}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(tond_common::ServerSettings::default())),
        }
    }

    async fn status_of(state: &AppState, id: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, error FROM deployments WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("status: {e}"))
    }

    #[tokio::test]
    async fn ctx_moves_the_row_out_of_the_queue_and_holds_one_build_slot() {
        static Q: LazyLock<BuildQueue> = LazyLock::new(|| BuildQueue::new(1));
        let state = test_state().await;
        for id in ["x1", "x2", "x3"] {
            sqlx::query(
                "INSERT INTO deployments (id, app_id, status, started_at, created_at)
                 VALUES (?, 'app', 'queued', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            )
            .bind(id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        }
        let a = DeployCtx::with_queue("x1".into(), &Q);
        let b = DeployCtx::with_queue("x2".into(), &Q);
        let c = DeployCtx::with_queue("x3".into(), &Q);
        a.begin_build(&state)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        a.begin_build(&state)
            .await
            .unwrap_or_else(|e| panic!("idempotent: {e}"));
        assert_eq!(status_of(&state, "x1").await.0, "running");

        let blocked =
            tokio::time::timeout(std::time::Duration::from_millis(50), b.begin_build(&state)).await;
        assert!(blocked.is_err(), "only one build at a time");
        assert_eq!(status_of(&state, "x2").await.0, "queued");

        // A rollback builds nothing: it runs while the slot is taken.
        c.begin(&state).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(status_of(&state, "x3").await.0, "running");

        a.release_build();
        b.begin_build(&state)
            .await
            .unwrap_or_else(|e| panic!("slot freed: {e}"));
        assert_eq!(status_of(&state, "x2").await.0, "running");

        finish(&state, "x1", "succeeded", None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        finish(&state, "x1", "failed", Some("late"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            status_of(&state, "x1").await,
            ("succeeded".to_string(), None),
            "a final status is never overwritten"
        );
    }

    #[tokio::test]
    async fn submit_marks_the_superseded_row_and_a_deleted_app_fails_cleanly() {
        static Q: LazyLock<BuildQueue> = LazyLock::new(|| BuildQueue::new(1));
        let state = test_state().await;
        for id in ["s1", "s2", "s3"] {
            sqlx::query(
                "INSERT INTO deployments (id, app_id, status, started_at, created_at)
                 VALUES (?, 'gone', 'queued', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            )
            .bind(id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        }
        // Hold the runner off: pretend a deploy of the app is running.
        Q.admit(job("s1", "gone"));
        let _ = Q.next_job("gone");
        Q.leave_waiting("s1"); // it is building
        let first = submit(&state, &Q, job("s2", "gone")).await;
        let second = submit(&state, &Q, job("s3", "gone")).await;
        assert_eq!(first.queue_position, 1);
        assert_eq!(second.superseded.as_deref(), Some("s2"));
        let (status, error) = status_of(&state, "s2").await;
        assert_eq!(status, "superseded");
        assert!(error.is_some_and(|e| e.contains("newer deploy")));

        // Let the runner go: the app row does not exist.
        let ctx = DeployCtx::with_queue("s3".into(), &Q);
        execute(&state, &ctx, &job("s3", "gone")).await;
        assert_eq!(
            status_of(&state, "s3").await,
            (
                "failed".to_string(),
                Some("The app was deleted".to_string())
            )
        );
    }
}
