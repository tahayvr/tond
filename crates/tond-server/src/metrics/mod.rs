// metrics — resource metrics for every container Tond runs
//
// One background task (run_metrics_collector) does all of it:
//
//   Discovery  One `docker ps` at startup (and after the event stream
//              reconnects), then Docker's event stream keeps the set of live
//              containers current (start / die / oom / kill / destroy /
//              rename). No polling of dockerd per tick.
//   Ownership  Each container is mapped to the app, database, cron job or
//              preview it belongs to (container ids in the DB, compose project
//              labels, the `tond-cron-job` label, then container names).
//              Compose services and multi-container targets are summed.
//   Sampling   Every TICK (30 s) the task reads each container's cgroup v2
//              files and /proc/<pid>/net/dev (see cgroup.rs) — microseconds,
//              no dockerd load — computes CPU and network *rates* against the
//              previous reading, and writes one small row per target in a
//              single transaction (store.rs). The host gauges are sampled in
//              the same tick and cached for /api/system/metrics (host.rs).
//   Rollups    When a 5-minute / 1-hour bucket closes, it is rolled up.
//              Retention is the janitor's job (store::prune).
//   Events     Restarts (a container that exited on its own and was started
//              again), crashes (exited on its own and stayed down) and OOM
//              kills are recorded in metric_events and wake the alert
//              engine (OOM and crash-loop rules read them). Exits that
//              follow a `kill`/`stop` request (deploys, stop, restart buttons)
//              are not crashes. Counter resets and memory.events oom_kill
//              deltas catch what the event stream missed.

pub mod cgroup;
pub mod host;
pub mod store;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use bollard::models::EventMessage;
use bollard::query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder};
use futures::{Stream, StreamExt};
use sqlx::SqlitePool;

use crate::runtime::changes;
use crate::{alerts, auth::secret_file, state::AppState};
pub use store::{EventKind, Owner, Sample, TargetKind};

/// Sampling interval.
pub const TICK: Duration = Duration::from_secs(30);
/// Delay before the first tick, so host CPU has a baseline to diff against.
const FIRST_TICK: Duration = Duration::from_secs(5);
/// A die this soon after a kill/stop request was requested, not a crash.
const REQUESTED_WINDOW_SECS: i64 = 60;
/// An unrequested exit not followed by a start within this long is a crash.
const CRASH_AFTER_SECS: i64 = 60;
const EVENT_RETRY_MIN: Duration = Duration::from_secs(5);
const EVENT_RETRY_MAX: Duration = Duration::from_secs(300);
/// Label Tond puts on cron containers (value: the cron job id).
pub const CRON_JOB_LABEL: &str = "tond-cron-job";

// ── Shared read-side state (for handlers and alerts) ──

/// Most recent sample of one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Current {
    pub ts: i64,
    pub sample: Sample,
    pub mem_peak: Option<i64>,
}

static CURRENT: RwLock<Option<HashMap<Owner, Current>>> = RwLock::new(None);
static CONTAINERS_RUNNING: AtomicI64 = AtomicI64::new(0);

/// Latest in-memory sample for `owner` (None before its first tick).
pub fn current(owner: &Owner) -> Option<Current> {
    CURRENT
        .read()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.get(owner).copied()))
}

/// Running containers Tond manages, as of the last tick / event.
pub fn containers_running() -> i64 {
    CONTAINERS_RUNNING.load(Ordering::Relaxed)
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── Ownership ──

/// Lookup tables mapping containers to owners, reloaded from the DB each tick.
#[derive(Debug, Default)]
pub struct Resolver {
    by_container: HashMap<String, Owner>,
    app_by_name: HashMap<String, String>,
    db_by_name: HashMap<String, String>,
    preview_by_slug: HashMap<String, String>,
    cron_ids: Vec<String>,
    names: HashMap<Owner, String>,
    loaded_at: Option<Instant>,
}

impl Resolver {
    pub async fn load(db: &SqlitePool) -> sqlx::Result<Self> {
        let mut r = Resolver {
            loaded_at: Some(Instant::now()),
            ..Default::default()
        };
        let apps: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, name, container_id FROM apps")
                .fetch_all(db)
                .await?;
        for (id, name, container) in apps {
            let owner = Owner::new(TargetKind::App, &id);
            if let Some(c) = container {
                r.by_container.insert(c, owner.clone());
            }
            r.app_by_name.insert(name.to_ascii_lowercase(), id);
            r.names.insert(owner, name);
        }
        let dbs: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, name, container_id FROM databases")
                .fetch_all(db)
                .await?;
        for (id, name, container) in dbs {
            let owner = Owner::new(TargetKind::Database, &id);
            if let Some(c) = container {
                r.by_container.insert(c, owner.clone());
            }
            r.names.insert(owner, name.clone());
            r.db_by_name.insert(name, id);
        }
        let previews: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
            "SELECT p.id, p.slug, p.container_id, a.name FROM preview_deployments p
             JOIN apps a ON a.id = p.app_id WHERE p.status != 'expired'",
        )
        .fetch_all(db)
        .await?;
        for (id, slug, container, app_name) in previews {
            let owner = Owner::new(TargetKind::Preview, &id);
            if let Some(c) = container {
                r.by_container.insert(c, owner.clone());
            }
            r.preview_by_slug.insert(slug.clone(), id);
            r.names.insert(owner, format!("{app_name} preview {slug}"));
        }
        let crons: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM cron_jobs")
            .fetch_all(db)
            .await?;
        for (id, name) in crons {
            r.names.insert(Owner::new(TargetKind::Cron, &id), name);
            r.cron_ids.push(id);
        }
        Ok(r)
    }

    /// Human-readable name of `owner` (its id when unknown).
    pub fn name(&self, owner: &Owner) -> String {
        name_of(&self.names, owner)
    }

    /// Map a container to its owner.
    pub fn resolve(&self, id: &str, name: &str, labels: &HashMap<String, String>) -> Option<Owner> {
        if let Some(job) = labels.get(CRON_JOB_LABEL) {
            return Some(Owner::new(TargetKind::Cron, job));
        }
        if let Some(project) = labels.get("com.docker.compose.project")
            && let Some(app) = self.app_by_name.get(project)
        {
            return Some(Owner::new(TargetKind::App, app));
        }
        if let Some(owner) = self.by_container.get(id) {
            return Some(owner.clone());
        }
        let name = name.trim_start_matches('/');
        let rest = name.strip_prefix("tond-")?;
        if let Some(job_run) = rest.strip_prefix("cron-") {
            let prefix = job_run.split('-').next().unwrap_or_default();
            if prefix.len() == 8 {
                return self
                    .cron_ids
                    .iter()
                    .find(|id| id.starts_with(prefix))
                    .map(|id| Owner::new(TargetKind::Cron, id));
            }
        }
        if let Some(db) = rest.strip_prefix("db-")
            && let Some(id) = self.db_by_name.get(db)
        {
            return Some(Owner::new(TargetKind::Database, id));
        }
        if let Some(slug) = rest.strip_prefix("preview-")
            && let Some(id) = self.preview_by_slug.get(slug)
        {
            return Some(Owner::new(TargetKind::Preview, id));
        }
        let app = rest.strip_suffix("-next").unwrap_or(rest);
        self.app_by_name
            .get(&app.to_ascii_lowercase())
            .map(|id| Owner::new(TargetKind::App, id))
    }
}

fn name_of(names: &HashMap<Owner, String>, owner: &Owner) -> String {
    names
        .get(owner)
        .cloned()
        .unwrap_or_else(|| owner.id.clone())
}

// ── Per-container tracking ──

#[derive(Debug, Clone, Copy)]
struct Counters {
    cpu_usec: u64,
    rx: u64,
    tx: u64,
    oom_kills: u64,
    at: Instant,
}

#[derive(Debug, Default)]
struct Tracked {
    name: String,
    labels: HashMap<String, String>,
    owner: Option<Owner>,
    running: bool,
    cgroup_dir: Option<PathBuf>,
    /// Docker was asked for this container's pid (cgroup fallback) already.
    inspected: bool,
    prev: Option<Counters>,
    /// Unix time of the last kill/stop request.
    kill_requested_at: Option<i64>,
    /// An exit nobody asked for: (unix time, detail).
    pending_exit: Option<(i64, String)>,
    last_oom_at: Option<i64>,
    last_restart_at: Option<i64>,
    restart_count: Option<i64>,
}

/// A restart / crash / OOM to persist (and maybe alert on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Happening {
    pub owner: Owner,
    pub ts: i64,
    pub kind: EventKind,
    pub detail: Option<String>,
}

/// Per-owner sum over its containers for one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aggregate {
    pub sample: Sample,
    /// At least one container had a previous reading (rates are valid).
    pub has_rates: bool,
    /// Summed memory limit; None if any container is unlimited.
    pub mem_limit: Option<i64>,
    pub mem_peak: Option<i64>,
}

/// The collector's state. Owned by the collector task.
pub struct Collector {
    cgroup_root: PathBuf,
    proc_root: PathBuf,
    tracked: HashMap<String, Tracked>,
    resolver: Resolver,
    target_ids: HashMap<Owner, i64>,
    mem_limits: HashMap<i64, i64>,
    next_5m: Option<i64>,
    next_1h: Option<i64>,
    /// Changes to the owner tables (runtime::changes): the resolver is
    /// reloaded only when one moved, not on every tick.
    owner_changes: Vec<tokio::sync::watch::Receiver<u64>>,
}

/// Reload the owner tables at least this often even without a change
/// notification (writes behind tond-server's back, e.g. sqlite3 by hand).
const OWNER_RELOAD_MAX_AGE: Duration = Duration::from_secs(3600);

impl Collector {
    pub fn new(cgroup_root: PathBuf, proc_root: PathBuf) -> Self {
        Self {
            cgroup_root,
            proc_root,
            tracked: HashMap::new(),
            resolver: Resolver::default(),
            target_ids: HashMap::new(),
            mem_limits: HashMap::new(),
            next_5m: None,
            next_1h: None,
            owner_changes: [
                changes::Table::Apps,
                changes::Table::Databases,
                changes::Table::CronJobs,
                changes::Table::PreviewDeployments,
            ]
            .into_iter()
            .map(changes::subscribe)
            .collect(),
        }
    }

    /// Whether the owner tables may have changed since the last load.
    fn owners_stale(&mut self) -> bool {
        let changed = self
            .owner_changes
            .iter()
            .any(|rx| rx.has_changed().unwrap_or(true));
        for rx in &mut self.owner_changes {
            rx.mark_unchanged();
        }
        changed
            || self
                .resolver
                .loaded_at
                .is_none_or(|t| t.elapsed() > OWNER_RELOAD_MAX_AGE)
    }

    fn publish_running_count(&self) {
        let n = self
            .tracked
            .values()
            .filter(|t| t.running && t.owner.is_some())
            .count();
        CONTAINERS_RUNNING.store(n as i64, Ordering::Relaxed);
    }

    /// Start tracking a container seen by `docker ps` or a start event.
    fn track(&mut self, id: &str, name: &str, labels: HashMap<String, String>) -> &mut Tracked {
        let owner = self.resolver.resolve(id, name, &labels);
        let entry = self.tracked.entry(id.to_string()).or_default();
        entry.name = name.trim_start_matches('/').to_string();
        entry.labels = labels;
        if entry.owner.is_none() {
            entry.owner = owner;
        }
        entry
    }

    /// Apply one Docker event. Returns what should be recorded.
    pub fn apply_event(
        &mut self,
        action: &str,
        id: &str,
        attrs: &HashMap<String, String>,
        now: i64,
    ) -> Option<Happening> {
        let name = attrs.get("name").cloned().unwrap_or_default();
        let happening = match action {
            "start" => {
                let known = self.tracked.contains_key(id);
                let entry = self.track(id, &name, attrs.clone());
                let was_running = entry.running;
                entry.running = true;
                entry.prev = None;
                entry.cgroup_dir = None;
                entry.inspected = false;
                let exit = entry.pending_exit.take();
                entry.kill_requested_at = None;
                match (known && !was_running, exit, entry.owner.clone()) {
                    (true, Some((_, detail)), Some(owner)) => {
                        entry.last_restart_at = Some(now);
                        Some(Happening {
                            owner,
                            ts: now,
                            kind: EventKind::Restart,
                            detail: Some(detail),
                        })
                    }
                    _ => None,
                }
            }
            "kill" | "stop" => {
                if let Some(entry) = self.tracked.get_mut(id) {
                    entry.kill_requested_at = Some(now);
                }
                None
            }
            "die" => {
                let entry = self.tracked.get_mut(id)?;
                entry.running = false;
                entry.prev = None;
                let requested = entry
                    .kill_requested_at
                    .is_some_and(|t| now - t <= REQUESTED_WINDOW_SECS);
                entry.kill_requested_at = None;
                if !requested {
                    let code = attrs
                        .get("exitCode")
                        .cloned()
                        .unwrap_or_else(|| "?".to_string());
                    let oom = entry.last_oom_at.is_some_and(|t| now - t <= 10);
                    let detail = if oom {
                        format!("exit code {code}, out of memory")
                    } else {
                        format!("exit code {code}")
                    };
                    entry.pending_exit = Some((now, detail));
                }
                None
            }
            "oom" => {
                let entry = self.tracked.get_mut(id)?;
                entry.last_oom_at = Some(now);
                entry.owner.clone().map(|owner| Happening {
                    owner,
                    ts: now,
                    kind: EventKind::Oom,
                    detail: None,
                })
            }
            "destroy" => {
                let entry = self.tracked.remove(id)?;
                match (entry.pending_exit, entry.owner) {
                    (Some((ts, detail)), Some(owner)) => Some(Happening {
                        owner,
                        ts,
                        kind: EventKind::Crash,
                        detail: Some(detail),
                    }),
                    _ => None,
                }
            }
            "rename" => {
                if let Some(entry) = self.tracked.get_mut(id) {
                    entry.name = name.trim_start_matches('/').to_string();
                }
                None
            }
            _ => None,
        };
        self.publish_running_count();
        happening
    }

    /// Crashes: unrequested exits that were not followed by a start.
    fn overdue_crashes(&mut self, now: i64) -> Vec<Happening> {
        let mut out = Vec::new();
        for t in self.tracked.values_mut() {
            if t.running {
                continue;
            }
            if let Some((ts, _)) = &t.pending_exit
                && now - ts >= CRASH_AFTER_SECS
                && let Some((ts, detail)) = t.pending_exit.take()
                && let Some(owner) = t.owner.clone()
            {
                out.push(Happening {
                    owner,
                    ts,
                    kind: EventKind::Crash,
                    detail: Some(detail),
                });
            }
        }
        out
    }

    /// Re-resolve containers whose owner was unknown (e.g. the DB row was
    /// written after the container started).
    fn resolve_unowned(&mut self) {
        for (id, t) in self.tracked.iter_mut() {
            if t.owner.is_none() {
                t.owner = self.resolver.resolve(id, &t.name, &t.labels);
            }
        }
    }

    /// Read every running, owned container and aggregate per owner.
    /// `pid_hint` supplies a pid for containers whose cgroup could not be
    /// found by probing (Docker inspect fallback).
    pub fn sample_containers(
        &mut self,
        now: i64,
        at: Instant,
        pid_hint: &HashMap<String, u32>,
    ) -> (HashMap<Owner, Aggregate>, Vec<Happening>) {
        let mut aggregates: HashMap<Owner, Aggregate> = HashMap::new();
        let mut happenings = Vec::new();
        for (id, t) in self.tracked.iter_mut() {
            let Some(owner) = t.owner.clone() else {
                continue;
            };
            if !t.running {
                continue;
            }
            if t.cgroup_dir.is_none() {
                t.cgroup_dir = cgroup::find_cgroup_dir(&self.cgroup_root, id).or_else(|| {
                    pid_hint.get(id).and_then(|pid| {
                        cgroup::cgroup_dir_for_pid(&self.cgroup_root, &self.proc_root, *pid)
                    })
                });
            }
            let Some(dir) = t.cgroup_dir.clone() else {
                continue;
            };
            let reading = match cgroup::read_cgroup(&dir) {
                Ok(r) => r,
                Err(_) => {
                    // Cgroup gone (container stopped between events): re-probe later.
                    t.cgroup_dir = None;
                    t.prev = None;
                    continue;
                }
            };
            let (rx, tx) = reading
                .first_pid
                .and_then(|pid| cgroup::read_net_dev(&self.proc_root, pid).ok())
                .unwrap_or((0, 0));

            let agg = aggregates
                .entry(owner.clone())
                .or_insert_with(|| Aggregate {
                    mem_limit: Some(0),
                    ..Default::default()
                });
            agg.sample.mem += reading.mem as i64;
            agg.sample.pids += reading.pids as i64;
            agg.mem_limit = match (agg.mem_limit, reading.mem_limit) {
                (Some(sum), Some(l)) => Some(sum + l as i64),
                _ => None,
            };
            if let Some(peak) = reading.mem_peak {
                agg.mem_peak = Some(agg.mem_peak.unwrap_or(0) + peak as i64);
            }

            if let Some(prev) = t.prev {
                let elapsed = at.duration_since(prev.at).as_secs_f64();
                if reading.cpu_usage_usec < prev.cpu_usec {
                    // Counters reset: the container restarted without us
                    // seeing the events (event stream down).
                    let recent = t
                        .last_restart_at
                        .is_some_and(|r| now - r < 2 * TICK.as_secs() as i64);
                    if !recent {
                        t.last_restart_at = Some(now);
                        happenings.push(Happening {
                            owner: owner.clone(),
                            ts: now,
                            kind: EventKind::Restart,
                            detail: Some("counters reset".to_string()),
                        });
                    }
                } else if elapsed > 0.0 {
                    let cpu = (reading.cpu_usage_usec - prev.cpu_usec) as f64 / (elapsed * 1e6);
                    agg.sample.cpu += (cpu * 10_000.0).round() as i64;
                    agg.sample.rx += (rx.saturating_sub(prev.rx) as f64 / elapsed).round() as i64;
                    agg.sample.tx += (tx.saturating_sub(prev.tx) as f64 / elapsed).round() as i64;
                    agg.has_rates = true;
                }
                if reading.oom_kills > prev.oom_kills {
                    let seen = t
                        .last_oom_at
                        .is_some_and(|o| now - o <= TICK.as_secs() as i64 + 5);
                    if !seen {
                        t.last_oom_at = Some(now);
                        let n = reading.oom_kills - prev.oom_kills;
                        happenings.push(Happening {
                            owner: owner.clone(),
                            ts: now,
                            kind: EventKind::Oom,
                            detail: Some(format!(
                                "{n} process{} killed",
                                if n == 1 { "" } else { "es" }
                            )),
                        });
                    }
                }
            }
            t.prev = Some(Counters {
                cpu_usec: reading.cpu_usage_usec,
                rx,
                tx,
                oom_kills: reading.oom_kills,
                at,
            });
        }
        (aggregates, happenings)
    }

    async fn target_id(&mut self, db: &SqlitePool, owner: &Owner) -> sqlx::Result<i64> {
        if let Some(id) = self.target_ids.get(owner) {
            return Ok(*id);
        }
        let id = store::ensure_target(db, owner).await?;
        self.target_ids.insert(owner.clone(), id);
        Ok(id)
    }

    /// Persist a happening and let the alert engine look at it.
    async fn record(&mut self, state: &AppState, h: Happening) {
        let target = match self.target_id(&state.db, &h.owner).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("metrics: cannot record {:?}: {}", h.kind, e);
                return;
            }
        };
        if let Err(e) =
            store::record_event(&state.db, target, h.ts, h.kind, h.detail.as_deref()).await
        {
            tracing::warn!("metrics: cannot record {:?}: {}", h.kind, e);
        }
        let name = self.resolver.name(&h.owner);
        tracing::info!("{} {} {}", h.owner.kind.as_str(), name, describe(&h));
        alerts::kick();
    }

    /// One sampling pass: host gauges, container samples, rollups.
    pub async fn tick(&mut self, state: &AppState, host: &mut host::HostSampler) {
        let now = now_unix();
        let at = Instant::now();
        host.sample(now);

        // Idle servers tick with nothing new: skip re-reading four tables.
        if self.owners_stale() {
            match Resolver::load(&state.db).await {
                Ok(r) => self.resolver = r,
                Err(e) => tracing::warn!("metrics: cannot load owners: {}", e),
            }
        }
        self.resolve_unowned();

        // Docker inspect only for containers whose cgroup probing failed, once.
        let mut pid_hint = HashMap::new();
        let unmapped: Vec<String> = self
            .tracked
            .iter()
            .filter(|(id, t)| {
                t.running
                    && t.owner.is_some()
                    && t.cgroup_dir.is_none()
                    && !t.inspected
                    && cgroup::find_cgroup_dir(&self.cgroup_root, id).is_none()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in unmapped {
            if let Some(t) = self.tracked.get_mut(&id) {
                t.inspected = true;
            }
            if let Ok(info) = state.docker.inspect_container(&id, None).await
                && let Some(pid) = info.state.and_then(|s| s.pid).filter(|p| *p > 0)
            {
                pid_hint.insert(id, pid as u32);
            }
        }

        let (aggregates, mut happenings) = self.sample_containers(now, at, &pid_hint);
        happenings.extend(self.overdue_crashes(now));

        let mut rows = Vec::new();
        let mut current = HashMap::new();
        for (owner, agg) in aggregates {
            let target = match self.target_id(&state.db, &owner).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("metrics: target for {:?}: {}", owner, e);
                    continue;
                }
            };
            let limit = agg.mem_limit.unwrap_or(0);
            let changed = self.mem_limits.get(&target) != Some(&limit);
            if agg.has_rates {
                if changed {
                    self.mem_limits.insert(target, limit);
                }
                rows.push(store::TickRow {
                    target,
                    sample: agg.sample,
                    mem_limit: changed.then_some(limit),
                });
            }
            current.insert(
                owner,
                Current {
                    ts: now,
                    sample: agg.sample,
                    mem_peak: agg.mem_peak,
                },
            );
        }
        if let Err(e) = store::insert_tick(&state.db, now, &rows).await {
            tracing::warn!("metrics: cannot store samples: {}", e);
        }
        if let Ok(mut c) = CURRENT.write() {
            *c = Some(current);
        }
        self.publish_running_count();

        for h in happenings {
            self.record(state, h).await;
        }

        if let Err(e) = self.roll_up(&state.db, now).await {
            tracing::warn!("metrics: rollup failed: {}", e);
        }

        // Alert rules are evaluated on this tick (no loop of their own).
        alerts::on_metrics_tick(state, now).await;
    }

    async fn roll_up(&mut self, db: &SqlitePool, now: i64) -> sqlx::Result<()> {
        let bucket_5m = now / 300 * 300;
        let next_5m = match self.next_5m {
            Some(n) => n,
            None => store::latest_bucket(db, store::RollupTable::FiveMinutes)
                .await?
                .map(|t| t + 300)
                .unwrap_or((now - store::RAW_KEEP_SECS) / 300 * 300),
        };
        if bucket_5m > next_5m {
            store::rollup_5m(db, next_5m, bucket_5m).await?;
            self.next_5m = Some(bucket_5m);
        } else {
            self.next_5m = Some(next_5m);
        }

        let bucket_1h = now / 3600 * 3600;
        let next_1h = match self.next_1h {
            Some(n) => n,
            None => store::latest_bucket(db, store::RollupTable::OneHour)
                .await?
                .map(|t| t + 3600)
                .unwrap_or((now - store::M5_KEEP_SECS) / 3600 * 3600),
        };
        if bucket_1h > next_1h {
            store::rollup_1h(db, next_1h, bucket_1h).await?;
            self.next_1h = Some(bucket_1h);
        } else {
            self.next_1h = Some(next_1h);
        }
        Ok(())
    }

    /// Rebuild the live set from `docker ps` (startup / after reconnecting).
    /// Restarts that happened while events were unavailable are caught by
    /// comparing Docker's RestartCount.
    async fn resync(&mut self, state: &AppState) -> anyhow::Result<()> {
        if let Ok(r) = Resolver::load(&state.db).await {
            self.resolver = r;
        }
        let list = state
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default().all(false).build(),
            ))
            .await?;
        let mut seen = Vec::with_capacity(list.len());
        let now = now_unix();
        let mut happenings = Vec::new();
        for c in list {
            let Some(id) = c.id else { continue };
            let name = c
                .names
                .and_then(|n| n.into_iter().next())
                .unwrap_or_default();
            let labels = c.labels.unwrap_or_default();
            let entry = self.track(&id, &name, labels);
            entry.running = true;
            entry.pending_exit = None;
            if entry.owner.is_some() {
                let previous = entry.restart_count;
                let owner = entry.owner.clone();
                let info = state.docker.inspect_container(&id, None).await.ok();
                let count = info.as_ref().and_then(|i| i.restart_count);
                if let Some(entry) = self.tracked.get_mut(&id) {
                    entry.restart_count = count;
                    if let (Some(before), Some(after), Some(owner)) = (previous, count, owner)
                        && after > before
                    {
                        entry.last_restart_at = Some(now);
                        happenings.push(Happening {
                            owner,
                            ts: now,
                            kind: EventKind::Restart,
                            detail: Some(format!(
                                "restarted {} time{} while events were unavailable",
                                after - before,
                                if after - before == 1 { "" } else { "s" }
                            )),
                        });
                    }
                }
            }
            seen.push(id);
        }
        self.tracked.retain(|id, _| seen.contains(id));
        self.publish_running_count();
        for h in happenings {
            self.record(state, h).await;
        }
        Ok(())
    }
}

/// What happened, for logs and alert messages ("restarted after …").
pub fn describe(h: &Happening) -> String {
    let what = match h.kind {
        EventKind::Restart => "restarted after exiting unexpectedly",
        EventKind::Crash => "exited unexpectedly and is not running",
        EventKind::Oom => "had a process killed for running out of memory",
    };
    match &h.detail {
        Some(d) => format!("{what} ({d})"),
        None => what.to_string(),
    }
}

type EventStream = Pin<Box<dyn Stream<Item = Result<EventMessage, bollard::errors::Error>> + Send>>;

fn subscribe(state: &AppState) -> EventStream {
    let filters = HashMap::from([
        ("type", vec!["container"]),
        (
            "event",
            vec!["start", "die", "oom", "kill", "stop", "destroy", "rename"],
        ),
    ]);
    let opts = EventsOptionsBuilder::default().filters(&filters).build();
    Box::pin(state.docker.events(Some(opts)))
}

/// Paths the collector reads (overridable for development).
fn roots() -> (PathBuf, PathBuf) {
    let cg = std::env::var("TOND_CGROUP_ROOT").unwrap_or_else(|_| "/sys/fs/cgroup".into());
    let proc = std::env::var("TOND_PROC_ROOT").unwrap_or_else(|_| "/proc".into());
    (PathBuf::from(cg), PathBuf::from(proc))
}

/// Spawned once at startup.
pub async fn run_metrics_collector(state: AppState) {
    let (cg, proc_root) = roots();
    if !Path::new(&cg).join("cgroup.controllers").is_file() {
        tracing::warn!(
            "metrics: {} is not a cgroup v2 mount; container metrics are unavailable",
            cg.display()
        );
    }
    let mut collector = Collector::new(cg, proc_root);
    let mut host = host::HostSampler::new(secret_file::data_dir());
    host.sample(now_unix());

    let mut events: Option<EventStream> = None;
    let mut retry = EVENT_RETRY_MIN;
    let mut reconnect_at = tokio::time::Instant::now();
    let lp = crate::runtime::loops::register("metrics");
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + FIRST_TICK, TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // Loop health for GET /api/system/self (runtime::loops).
                lp.woke();
                let _run = lp.run();
                collector.tick(&state, &mut host).await;
            }
            _ = tokio::time::sleep_until(reconnect_at), if events.is_none() => {
                events = Some(subscribe(&state));
                match collector.resync(&state).await {
                    Ok(()) => retry = EVENT_RETRY_MIN,
                    Err(e) => {
                        tracing::warn!("metrics: cannot list containers: {}", e);
                        events = None;
                        reconnect_at = tokio::time::Instant::now() + retry;
                        retry = (retry * 2).min(EVENT_RETRY_MAX);
                    }
                }
            }
            ev = next_event(&mut events) => match ev {
                Some(Ok(msg)) => {
                    let action = msg.action.unwrap_or_default();
                    let actor = msg.actor.unwrap_or_default();
                    let id = actor.id.unwrap_or_default();
                    let attrs = actor.attributes.unwrap_or_default();
                    // Unknown container starting: refresh the owner tables once
                    // so a just-deployed app is attributed immediately.
                    if action == "start" && !collector.tracked.contains_key(&id)
                        && collector.resolver.resolve(&id, attrs.get("name").map(String::as_str).unwrap_or(""), &attrs).is_none()
                        && collector.resolver.loaded_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5))
                        && let Ok(r) = Resolver::load(&state.db).await
                    {
                        collector.resolver = r;
                    }
                    let ts = msg.time.unwrap_or_else(now_unix);
                    if let Some(h) = collector.apply_event(&action, &id, &attrs, ts) {
                        collector.record(&state, h).await;
                    }
                }
                Some(Err(e)) => {
                    tracing::debug!("metrics: Docker event stream error: {}", e);
                    events = None;
                    reconnect_at = tokio::time::Instant::now() + retry;
                    retry = (retry * 2).min(EVENT_RETRY_MAX);
                }
                None => {
                    events = None;
                    reconnect_at = tokio::time::Instant::now() + retry;
                    retry = (retry * 2).min(EVENT_RETRY_MAX);
                }
            }
        }
    }
}

async fn next_event(
    events: &mut Option<EventStream>,
) -> Option<Result<EventMessage, bollard::errors::Error>> {
    match events {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
