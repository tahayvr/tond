// uptime — uptime checks of every public URL, and the outbound heartbeat
//
// One background loop (`run`) does both. Checks are scheduled on a shared
// minute grid: every due time is a whole minute (+ GRID_OFFSET), so N apps
// probed every minute cost ONE wakeup a minute, not N. At each wakeup every
// due check is probed concurrently (at most CONCURRENCY at a time) through
// the public path (probe.rs: DNS → TCP to the public address → TLS → HTTP),
// the round's results are written in one statement, and incidents open and
// close. The heartbeat (heartbeat.rs) is due on the same grid. With nothing
// to check the loop sleeps until a change (route_state — apps added, removed,
// renamed, started or stopped; the dashboard domain — or uptime_checks, or a
// heartbeat settings change).
//
// Which checks run: every app with a domain (a check row is created for it
// automatically) and the dashboard (when a dashboard domain is set). Apps
// that are stopped, never deployed or serve no traffic (background workers)
// are paused: not probed, and an open incident is closed.
//
// A failed probe is retried once after RETRY_AFTER before it counts, so a
// single dropped packet doesn't open an incident.
//
// ── Events (for the alert engine) ───────────────────────────────────────────
// A check going down or coming back up is published two ways:
//   1. in-process: `uptime::subscribe()` returns a tokio broadcast receiver of
//      `UptimeEvent { kind: Down | Up, check_id, app_id, name, host, cause,
//      detail, at, incident_id, down_secs }` — the way to react (alerts).
//      Sent after the incident row is written, so a subscriber can always
//      look the incident up. A lagging receiver loses events (capacity 64);
//      catch up from the table.
//   2. durable: uptime_incidents — a new row is a "down" event, `ended_at`
//      being set is the matching "up" event (history, catching up after a
//      restart). It is deliberately not a runtime::changes watched table:
//      every round would wake every watcher.
// Transitions are also logged (one line each). Pausing a check (app stopped,
// check turned off) closes its incident and sends an Up event whose detail
// says why.
//
// Storage and retention: see store.rs (1-minute rows 48 h, hourly 90 days,
// incidents 90 days). The janitor calls store::prune.

pub mod heartbeat;
pub mod probe;
pub mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use futures::StreamExt;
use serde::Serialize;
use tokio::sync::{Mutex, broadcast};

use crate::runtime::changes::{Table, Watch};
use crate::runtime::loops;
use crate::state::AppState;
use probe::{Cause, DnsState, ProbeResult, ProbeSpec, Prober};

/// Probes running at once.
const CONCURRENCY: usize = 8;
/// Every due time is a whole minute plus this.
const GRID_OFFSET: i64 = 7;
/// A failed probe is tried again after this before it counts.
const RETRY_AFTER: Duration = Duration::from_secs(3);
/// Re-read the configuration at least this often (writes behind tond-server's
/// back, e.g. sqlite3 by hand).
const MAX_SLEEP: i64 = 3600;
pub const MIN_INTERVAL_SECS: i64 = 60;
pub const MAX_INTERVAL_SECS: i64 = 3600;

// ── Events ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Down,
    Up,
}

/// A check went down or came back up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UptimeEvent {
    pub kind: EventKind,
    pub check_id: i64,
    /// The app, for app checks (None for the dashboard).
    pub app_id: Option<String>,
    /// App name, or "Dashboard".
    pub name: String,
    pub host: String,
    /// Why it went down (for Up: why it had been down).
    pub cause: Cause,
    pub detail: String,
    /// Unix seconds.
    pub at: i64,
    pub incident_id: i64,
    /// How long it was down (Up events only).
    pub down_secs: Option<i64>,
}

static EVENTS: LazyLock<broadcast::Sender<UptimeEvent>> =
    LazyLock::new(|| broadcast::channel(64).0);

/// Receive every future down / up event (the alert engine's hook).
#[allow(dead_code)]
pub fn subscribe() -> broadcast::Receiver<UptimeEvent> {
    EVENTS.subscribe()
}

// ── Live state (read by the API) ──

/// The newest probe of a check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LastProbe {
    /// Unix seconds.
    pub at: i64,
    #[serde(flatten)]
    pub result: ProbeResult,
}

#[derive(Debug, Clone, Default)]
struct Live {
    last: Option<LastProbe>,
    /// Open incident: (id, started_at, cause)
    incident: Option<(i64, i64, Cause)>,
    next_due: Option<i64>,
}

static LIVE: LazyLock<RwLock<HashMap<i64, Live>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
/// Probes before this are in uptime_1h (the rollup cursor).
static ROLLED_UNTIL: AtomicI64 = AtomicI64::new(0);
/// When the next heartbeat is due (0 = none).
static NEXT_BEAT: AtomicI64 = AtomicI64::new(0);
/// Serialises recording (a round and "check now" never interleave).
static RECORD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// What the API shows about a check right now.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LiveStatus {
    pub last: Option<LastProbe>,
    pub incident_id: Option<i64>,
    pub down_since: Option<i64>,
    /// Unix seconds of the next scheduled probe.
    pub next_at: Option<i64>,
}

pub fn live_status(check_id: i64) -> LiveStatus {
    let live = LIVE.read().ok().and_then(|m| m.get(&check_id).cloned());
    match live {
        Some(l) => LiveStatus {
            incident_id: l.incident.map(|i| i.0),
            down_since: l.incident.map(|i| i.1),
            next_at: l.next_due,
            last: l.last,
        },
        None => LiveStatus::default(),
    }
}

/// When the next heartbeat will be sent (None when it is off).
pub fn next_heartbeat() -> Option<i64> {
    match NEXT_BEAT.load(Ordering::Relaxed) {
        0 => None,
        t => Some(t),
    }
}

/// Where hourly rollups end (see store::windows).
pub fn rolled_until(now: i64) -> i64 {
    match ROLLED_UNTIL.load(Ordering::Relaxed) {
        0 => now / 3600 * 3600,
        t => t,
    }
}

// ── Checks ──

/// A check ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: i64,
    pub app_id: Option<String>,
    pub name: String,
    /// Empty when there is nothing to probe (no domain).
    pub host: String,
    pub spec: ProbeSpec,
    pub interval_secs: i64,
    /// Probed (enabled, has a host, app running).
    pub active: bool,
    /// Why it is not probed.
    pub paused_reason: Option<&'static str>,
}

/// Turn stored rows into runnable checks.
pub fn build_checks(rows: Vec<store::CheckWithApp>, dashboard_domain: &str) -> Vec<Check> {
    rows.into_iter()
        .filter_map(|r| {
            let c = r.check;
            let (app_id, name, host, paused) = if c.kind == "dashboard" {
                let paused = dashboard_domain
                    .is_empty()
                    .then_some("No dashboard domain is set");
                (
                    None,
                    "Dashboard".to_string(),
                    dashboard_domain.to_string(),
                    paused,
                )
            } else {
                // The app is gone (the trigger removes its check shortly).
                let name = r.app_name?;
                let host = r.app_domain.unwrap_or_default();
                let status = r.app_status.unwrap_or_default();
                let paused = if host.is_empty() {
                    Some("The app has no domain")
                } else if r.app_readiness.as_deref() == Some("none") {
                    Some("Background workers serve no traffic")
                } else if status == "stopped" {
                    Some("The app is stopped")
                } else if status == "pending" {
                    Some("The app hasn't been deployed yet")
                } else {
                    None
                };
                (Some(c.owner_id.clone()), name, host, paused)
            };
            let paused = if c.enabled {
                paused
            } else {
                Some("Checks are turned off for this app")
            };
            Some(Check {
                id: c.id,
                app_id,
                name,
                spec: ProbeSpec {
                    host: host.clone(),
                    path: c.path,
                    expected_status: c.expected_status.and_then(|s| u16::try_from(s).ok()),
                    timeout: Duration::from_secs(c.timeout_secs.clamp(1, 60) as u64),
                },
                host,
                interval_secs: c.interval_secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS),
                active: paused.is_none(),
                paused_reason: paused,
            })
        })
        .collect()
}

async fn load(state: &AppState) -> anyhow::Result<Vec<Check>> {
    let rows = store::load_checks(&state.db).await?;
    let dashboard = state.settings.read().await.dashboard_domain.clone();
    Ok(build_checks(rows, &dashboard))
}

// ── Scheduling ──

/// The first grid point at or after `t`.
pub fn next_grid(t: i64) -> i64 {
    let base = t - GRID_OFFSET;
    (base + 59).div_euclid(60) * 60 + GRID_OFFSET
}

/// Next due time after a probe that was due at `due`, never in the past.
fn advance(due: i64, interval: i64, now: i64) -> i64 {
    (due + interval).max(next_grid(now))
}

// ── Probing and recording ──

/// Probe; on failure try once more before believing it.
async fn probe_confirmed(prober: &Prober, spec: &ProbeSpec) -> ProbeResult {
    let first = prober.probe(spec).await;
    if first.ok {
        return first;
    }
    tokio::time::sleep(RETRY_AFTER).await;
    prober.probe(spec).await
}

/// Probe `checks` concurrently.
async fn probe_all(checks: Vec<Check>) -> Vec<(Check, ProbeResult)> {
    if checks.is_empty() {
        return Vec::new();
    }
    let prober = Prober::new();
    let prober = &prober;
    futures::stream::iter(checks)
        .map(|c| async move {
            let r = probe_confirmed(prober, &c.spec).await;
            (c, r)
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await
}

/// Store results (stamped `ts`), open / close incidents, publish events.
async fn record(state: &AppState, ts: i64, results: &[(Check, ProbeResult)]) -> anyhow::Result<()> {
    let _guard = RECORD.lock().await;
    let rows: Vec<store::ResultRow> = results
        .iter()
        .map(|(c, r)| store::ResultRow {
            check_id: c.id,
            ts,
            result: r.code(),
            status: r.status.map(i64::from),
            latency_ms: Some(i64::from(r.latency_ms)),
        })
        .collect();
    store::insert_results(&state.db, &rows).await?;

    let now = chrono::Utc::now().timestamp();
    for (check, result) in results {
        let open = LIVE
            .read()
            .ok()
            .and_then(|m| m.get(&check.id).and_then(|l| l.incident));
        let mut incident = open;
        match (result.ok, open) {
            (false, None) => {
                let cause = result.cause.unwrap_or(Cause::Connect);
                let id =
                    store::open_incident(&state.db, check.id, ts, cause, &result.detail).await?;
                incident = Some((id, ts, cause));
                tracing::info!(
                    "uptime: {} ({}) is down: {}",
                    check.name,
                    check.host,
                    result.detail
                );
                publish(check, EventKind::Down, cause, &result.detail, ts, id, None);
            }
            (true, Some((id, started, cause))) => {
                store::close_incident(&state.db, id, ts).await?;
                incident = None;
                tracing::info!(
                    "uptime: {} ({}) is back up after {} s",
                    check.name,
                    check.host,
                    ts - started
                );
                publish(
                    check,
                    EventKind::Up,
                    cause,
                    &result.detail,
                    ts,
                    id,
                    Some(ts - started),
                );
            }
            _ => {}
        }
        if let Ok(mut m) = LIVE.write() {
            let live = m.entry(check.id).or_default();
            live.last = Some(LastProbe {
                at: now,
                result: result.clone(),
            });
            live.incident = incident;
        }
    }
    Ok(())
}

fn publish(
    check: &Check,
    kind: EventKind,
    cause: Cause,
    detail: &str,
    at: i64,
    incident_id: i64,
    down_secs: Option<i64>,
) {
    // No receivers is fine.
    let _ = EVENTS.send(UptimeEvent {
        kind,
        check_id: check.id,
        app_id: check.app_id.clone(),
        name: check.name.clone(),
        host: check.host.clone(),
        cause,
        detail: detail.to_string(),
        at,
        incident_id,
        down_secs,
    });
}

/// Close the incidents of checks that are no longer probed (stopped app,
/// disabled check, domain removed).
async fn close_paused(state: &AppState, checks: &[Check], now: i64) -> anyhow::Result<()> {
    let _guard = RECORD.lock().await;
    for check in checks.iter().filter(|c| !c.active) {
        let open = LIVE
            .read()
            .ok()
            .and_then(|m| m.get(&check.id).and_then(|l| l.incident));
        if let Some((id, started, cause)) = open {
            store::close_incident(&state.db, id, now).await?;
            if let Ok(mut m) = LIVE.write()
                && let Some(l) = m.get_mut(&check.id)
            {
                l.incident = None;
            }
            publish(
                check,
                EventKind::Up,
                cause,
                check.paused_reason.unwrap_or("paused"),
                now,
                id,
                Some(now - started),
            );
        }
    }
    Ok(())
}

/// Probe one check now (the "Check now" button) and record the result.
pub async fn check_now(state: &AppState, check_id: i64) -> anyhow::Result<LiveStatus> {
    let checks = load(state).await?;
    let Some(check) = checks.into_iter().find(|c| c.id == check_id) else {
        anyhow::bail!("unknown check");
    };
    if !check.active {
        anyhow::bail!(
            "{}",
            check.paused_reason.unwrap_or("This check is not running")
        );
    }
    let results = probe_all(vec![check]).await;
    let ts = chrono::Utc::now().timestamp() / 60 * 60;
    record(state, ts, &results).await?;
    Ok(live_status(check_id))
}

/// Restore open incidents after a restart.
async fn restore(state: &AppState) -> anyhow::Result<()> {
    let open = store::open_incidents(&state.db).await?;
    if let Ok(mut m) = LIVE.write() {
        for i in open {
            let cause = Cause::parse(&i.cause).unwrap_or(Cause::Connect);
            m.entry(i.check_id).or_default().incident = Some((i.id, i.started_at, cause));
        }
    }
    let now = chrono::Utc::now().timestamp();
    let rolled = store::latest_rollup(&state.db)
        .await?
        .map(|t| t + 3600)
        .unwrap_or((now - store::M1_KEEP_SECS) / 3600 * 3600);
    ROLLED_UNTIL.store(rolled, Ordering::Relaxed);
    Ok(())
}

async fn roll_up(state: &AppState, now: i64) -> anyhow::Result<()> {
    let hour = now / 3600 * 3600;
    let from = rolled_until(now);
    if hour > from {
        store::rollup_1h(&state.db, from, hour).await?;
        ROLLED_UNTIL.store(hour, Ordering::Relaxed);
    }
    Ok(())
}

/// Spawned once at startup.
pub async fn run(state: AppState) {
    let lp = loops::register("uptime");
    let mut watch = Watch::new(&[Table::RouteState, Table::Uptime]);
    if let Err(e) = restore(&state).await {
        tracing::warn!("uptime: cannot restore state: {e}");
    }
    let mut checks: Vec<Check> = Vec::new();
    let mut beat: Option<heartbeat::Config> = None;
    let mut beat_due: Option<i64> = None;
    let mut beat_state: Option<&'static str> = None;
    let mut reload = true;
    let mut loaded_at = std::time::Instant::now();

    loop {
        let now = chrono::Utc::now().timestamp();
        if reload || loaded_at.elapsed() > Duration::from_secs(MAX_SLEEP as u64) {
            reload = false;
            loaded_at = std::time::Instant::now();
            match load(&state).await {
                Ok(c) => checks = c,
                Err(e) => tracing::warn!("uptime: cannot load checks: {e}"),
            }
            if let Err(e) = close_paused(&state, &checks, now).await {
                tracing::warn!("uptime: cannot close incidents: {e}");
            }
            match heartbeat::load(&state.db).await {
                Ok(cfg) => {
                    let interval = cfg
                        .interval_secs
                        .clamp(heartbeat::MIN_INTERVAL_SECS, heartbeat::MAX_INTERVAL_SECS);
                    beat_due = cfg.active().then(|| {
                        beat_due
                            .filter(|d| *d <= next_grid(now) + interval)
                            .unwrap_or_else(|| next_grid(now + 1))
                    });
                    beat = Some(cfg);
                }
                Err(e) => tracing::warn!("uptime: cannot load the heartbeat settings: {e}"),
            }
            // Keep each check's schedule; new checks start at the next minute.
            if let Ok(mut m) = LIVE.write() {
                m.retain(|id, _| checks.iter().any(|c| c.id == *id));
                for c in &checks {
                    let live = m.entry(c.id).or_default();
                    live.next_due = if c.active {
                        Some(
                            live.next_due
                                .filter(|d| *d <= next_grid(now) + c.interval_secs)
                                .unwrap_or_else(|| next_grid(now + 1)),
                        )
                    } else {
                        None
                    };
                }
            }
        }

        NEXT_BEAT.store(beat_due.unwrap_or(0), Ordering::Relaxed);
        let next_check = LIVE
            .read()
            .ok()
            .and_then(|m| m.values().filter_map(|l| l.next_due).min());
        let wake_at = [next_check, beat_due].into_iter().flatten().min();
        let timeout = Duration::from_secs(
            wake_at
                .map(|t| (t - now).clamp(0, MAX_SLEEP))
                .unwrap_or(MAX_SLEEP) as u64,
        );
        if lp.wait(Some(timeout), &mut watch).await == loops::Wake::Event {
            reload = true;
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        if wake_at.is_none_or(|t| t > now) {
            // The periodic re-read (or a timer that fired early).
            reload = wake_at.is_none();
            continue;
        }
        let run = lp.run();

        // Due checks, stamped with the grid time they were due at.
        let mut due: Vec<Check> = Vec::new();
        let mut stamp = now;
        if let Ok(mut m) = LIVE.write() {
            for c in checks.iter().filter(|c| c.active) {
                if let Some(live) = m.get_mut(&c.id)
                    && let Some(d) = live.next_due
                    && d <= now
                {
                    stamp = stamp.min(d);
                    live.next_due = Some(advance(d, c.interval_secs, now));
                    due.push(c.clone());
                }
            }
        }
        let ts = stamp / 60 * 60;
        let results = probe_all(due).await;
        if !results.is_empty()
            && let Err(e) = record(&state, ts, &results).await
        {
            tracing::warn!("uptime: cannot record results: {e}");
            run.fail(&e);
            continue;
        }

        if let (Some(cfg), Some(d)) = (&beat, beat_due)
            && d <= now
        {
            let interval = cfg
                .interval_secs
                .clamp(heartbeat::MIN_INTERVAL_SECS, heartbeat::MAX_INTERVAL_SECS);
            beat_due = Some(advance(d, interval, chrono::Utc::now().timestamp()));
            // Log changes only (a failing monitor must not flood the journal).
            let outcome = heartbeat::beat(&state, cfg).await;
            let kind = match &outcome {
                heartbeat::Outcome::Sent { .. } => "sent",
                heartbeat::Outcome::Failed { .. } => "failed",
                heartbeat::Outcome::Skipped { .. } => "skipped",
            };
            if beat_state != Some(kind) {
                match &outcome {
                    heartbeat::Outcome::Sent { .. } if beat_state.is_some() => {
                        tracing::info!("heartbeat: pings are going through again")
                    }
                    heartbeat::Outcome::Sent { .. } => {}
                    heartbeat::Outcome::Failed { error } => {
                        tracing::warn!("heartbeat: ping failed: {error}")
                    }
                    heartbeat::Outcome::Skipped { reason } => {
                        tracing::warn!("heartbeat: not sent because {reason}")
                    }
                }
                beat_state = Some(kind);
            }
        }

        if let Err(e) = roll_up(&state, chrono::Utc::now().timestamp()).await {
            tracing::warn!("uptime: rollup failed: {e}");
            run.fail(&e);
        }
    }
}

/// Human text for the DNS state (for the API).
pub fn dns_label(dns: DnsState) -> &'static str {
    match dns {
        DnsState::Matches => "Points at this server",
        DnsState::Elsewhere => "Points somewhere else (a CDN or an old record)",
        DnsState::Unknown => "Can't tell whether it points here",
        DnsState::Failed => "Doesn't resolve",
        DnsState::Overridden => "Resolution overridden for tests",
    }
}

#[cfg(test)]
pub(crate) mod tests;
