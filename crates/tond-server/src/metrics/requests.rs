// metrics/requests.rs — request metrics from tond-proxy's per-route counters
//
// Collection (inside the metrics collector's 30 s tick, no loop of its own):
//   1. Ask the proxy for its cumulative counters (`traffic` over proxy.sock).
//   2. Diff against the previous reading: a new proxy pid, a new route
//      generation or a counter going backwards means the counters restarted,
//      so the new reading is the whole delta. The first reading after
//      tond-server starts is only a baseline.
//   3. Keep the last few deltas per route in memory (LIVE) for "right now"
//      numbers, and add them to the pending minute.
//   4. When a tick belongs to a later minute, write the pending minute's rows
//      (targets with requests only) to requests_1m and roll finished hours up
//      into requests_1h — all in one transaction, about once a minute.
//
// Retention (janitor, store::prune): requests_1m 48 h, requests_1h 90 days.
//
// Queries return at most ~480 buckets:
//   1h → 1m (60)   6h → 1m (360)   24h → 3 min (480)
//   7d → 1h (168)  30d → 2 h (360) 90d → 5 h (432)

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::{AssertSqlSafe, SqlitePool};
use tond_common::traffic::{
    RouteKind, TrafficCounters, TrafficSnapshot, decode_histogram, encode_histogram, percentile,
};
use tond_common::{RequestCurrent, RequestSeries, RequestSummary};

/// Kept for requests_1m / requests_1h.
pub const M1_KEEP_SECS: i64 = 48 * 3600;
pub const H1_KEEP_SECS: i64 = 90 * 86_400;
/// Deltas kept in memory per route (10 ticks = 5 minutes).
const LIVE_LEN: usize = 10;
/// "Right now" covers deltas newer than this.
const CURRENT_WINDOW_SECS: i64 = 75;

/// A route as tond-server knows it: (kind, id — "" for dashboard/unknown).
pub type RouteId = (RouteKind, String);

/// metric_targets.kind for a route kind.
pub fn kind_str(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::App => "app",
        RouteKind::Preview => "preview",
        RouteKind::Dashboard => "dashboard",
        RouteKind::Unknown => "unknown",
    }
}

// ── Live state (read by handlers) ──

#[derive(Debug, Clone)]
struct LiveEntry {
    /// End of the delta's interval, unix seconds.
    ts: i64,
    secs: i64,
    counters: TrafficCounters,
}

static LIVE: RwLock<Option<HashMap<RouteId, VecDeque<LiveEntry>>>> = RwLock::new(None);
static PROXY_CONNECTED: AtomicBool = AtomicBool::new(false);
/// The minute being accumulated (not written yet), so queries include it.
type PendingMinute = (i64, HashMap<RouteId, TrafficCounters>);
static PENDING: RwLock<Option<PendingMinute>> = RwLock::new(None);

/// The not-yet-written minute of `route`: (minute start, counters).
pub fn pending(route: &RouteId) -> Option<(i64, TrafficCounters)> {
    let pending = PENDING.read().ok()?;
    let (minute, rows) = pending.as_ref()?;
    rows.get(route).map(|c| (*minute, c.clone()))
}

/// Every route's not-yet-written minute.
pub fn pending_all() -> Option<PendingMinute> {
    PENDING.read().ok()?.clone()
}

/// Whether the last scrape reached the proxy.
pub fn proxy_connected() -> bool {
    PROXY_CONNECTED.load(Ordering::Relaxed)
}

/// Requests of the last minute or so, from memory.
pub fn current(route: &RouteId, now: i64) -> Option<RequestCurrent> {
    let live = LIVE.read().ok()?;
    let ring = live.as_ref()?.get(route)?;
    let mut sum = TrafficCounters::default();
    let mut secs = 0;
    let mut last = 0;
    for e in ring.iter().filter(|e| e.ts > now - CURRENT_WINDOW_SECS) {
        sum.add(&e.counters);
        secs += e.secs;
        last = last.max(e.ts);
    }
    if secs == 0 {
        return None;
    }
    Some(RequestCurrent {
        t: last,
        secs,
        rps: round2(sum.requests as f64 / secs as f64),
        err_pct: err_pct(&sum.status),
        p95: percentile(&sum.latency, 0.95),
    })
}

fn publish_live(deltas: &[(RouteId, TrafficCounters)], present: &[RouteId], ts: i64, secs: i64) {
    let Ok(mut live) = LIVE.write() else { return };
    let map = live.get_or_insert_with(HashMap::new);
    map.retain(|k, _| present.contains(k));
    for (route, delta) in deltas {
        let ring = map.entry(route.clone()).or_default();
        if ring.len() == LIVE_LEN {
            ring.pop_front();
        }
        ring.push_back(LiveEntry {
            ts,
            secs,
            counters: delta.clone(),
        });
    }
}

// ── Diffing ──

/// The collector's request state.
#[derive(Debug, Default)]
pub struct Tracker {
    last: HashMap<RouteId, (u64, TrafficCounters)>,
    last_pid: Option<u32>,
    last_at: Option<i64>,
    pending_minute: Option<i64>,
    pending: HashMap<RouteId, TrafficCounters>,
    next_1h: Option<i64>,
    target_ids: HashMap<RouteId, i64>,
}

fn route_id(kind: RouteKind, id: Option<String>) -> RouteId {
    (kind, id.unwrap_or_default())
}

impl Tracker {
    /// Diff a snapshot against the previous one. Returns the non-empty
    /// deltas and every route present in the snapshot.
    pub fn diff(
        &mut self,
        snap: TrafficSnapshot,
    ) -> (Vec<(RouteId, TrafficCounters)>, Vec<RouteId>) {
        // No baseline yet (tond-server just started): remember, report nothing.
        let baseline = self.last_pid.is_some();
        let restarted = self.last_pid.is_some_and(|p| p != snap.pid);
        let mut deltas = Vec::new();
        let mut present = Vec::with_capacity(snap.routes.len());
        let mut next = HashMap::with_capacity(snap.routes.len());
        for route in snap.routes {
            let id = route_id(route.kind, route.id);
            let delta = if !baseline {
                None
            } else if restarted {
                Some(route.counters.clone())
            } else {
                match self.last.get(&id) {
                    Some((generation, prev)) if *generation == route.generation => Some(
                        route
                            .counters
                            .since(prev)
                            .unwrap_or_else(|| route.counters.clone()),
                    ),
                    // New route (or re-created counters) since the last reading.
                    _ => Some(route.counters.clone()),
                }
            };
            if let Some(d) = delta.filter(|d| !d.is_empty()) {
                deltas.push((id.clone(), d));
            }
            present.push(id.clone());
            next.insert(id, (route.generation, route.counters));
        }
        self.last = next;
        self.last_pid = Some(snap.pid);
        (deltas, present)
    }

    /// Add deltas to the pending minute; returns a finished minute to write
    /// when `minute` is later than the pending one.
    pub fn accumulate(
        &mut self,
        minute: i64,
        deltas: &[(RouteId, TrafficCounters)],
    ) -> Option<(i64, HashMap<RouteId, TrafficCounters>)> {
        let finished = match self.pending_minute {
            Some(p) if minute > p => {
                let rows = std::mem::take(&mut self.pending);
                Some((p, rows))
            }
            _ => None,
        };
        if self.pending_minute.is_none_or(|p| minute > p) {
            self.pending_minute = Some(minute);
        }
        for (route, delta) in deltas {
            self.pending.entry(route.clone()).or_default().add(delta);
        }
        finished.filter(|(_, rows)| !rows.is_empty())
    }

    async fn target_id(&mut self, db: &SqlitePool, route: &RouteId) -> sqlx::Result<i64> {
        if let Some(id) = self.target_ids.get(route) {
            return Ok(*id);
        }
        let id = ensure_target(db, route).await?;
        self.target_ids.insert(route.clone(), id);
        Ok(id)
    }

    /// One scrape (metrics collector tick).
    pub async fn tick(&mut self, db: &SqlitePool, now: i64) {
        let Some(snap) = crate::proxy_link::traffic().await else {
            PROXY_CONNECTED.store(false, Ordering::Relaxed);
            return;
        };
        PROXY_CONNECTED.store(true, Ordering::Relaxed);
        let secs = self.last_at.map_or(30, |t| (now - t).clamp(1, 3600));
        self.last_at = Some(now);
        let (deltas, present) = self.diff(snap);
        publish_live(&deltas, &present, now, secs);

        // The delta covers (now - secs, now]: attribute it to the minute of
        // its midpoint, so 30 s ticks land two per minute.
        let minute = (now - secs / 2) / 60 * 60;
        if let Some((minute, rows)) = self.accumulate(minute, &deltas)
            && let Err(e) = self.write(db, minute, rows).await
        {
            tracing::warn!("metrics: cannot store request metrics: {e}");
        }
        if let (Some(minute), Ok(mut p)) = (self.pending_minute, PENDING.write()) {
            *p = Some((minute, self.pending.clone()));
        }
    }

    async fn write(
        &mut self,
        db: &SqlitePool,
        minute: i64,
        rows: HashMap<RouteId, TrafficCounters>,
    ) -> sqlx::Result<()> {
        let mut targeted = Vec::with_capacity(rows.len());
        for (route, counters) in rows {
            targeted.push((self.target_id(db, &route).await?, counters));
        }
        let next_1h = match self.next_1h {
            Some(n) => n,
            None => latest_hour(db)
                .await?
                .map(|t| t + 3600)
                .unwrap_or((minute - M1_KEEP_SECS) / 3600 * 3600),
        };
        let hour = minute / 3600 * 3600;
        let rollup = (hour > next_1h).then_some((next_1h, hour));
        write_minute(db, minute, &targeted, rollup).await?;
        self.next_1h = Some(next_1h.max(hour));
        Ok(())
    }
}

// ── Storage ──

/// Target id for a route (metric_targets, created on first use).
pub async fn ensure_target(db: &SqlitePool, route: &RouteId) -> sqlx::Result<i64> {
    let kind = kind_str(route.0);
    sqlx::query("INSERT INTO metric_targets (kind, owner_id) VALUES (?, ?) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(&route.1)
        .execute(db)
        .await?;
    sqlx::query_scalar("SELECT id FROM metric_targets WHERE kind = ? AND owner_id = ?")
        .bind(kind)
        .bind(&route.1)
        .fetch_one(db)
        .await
}

/// Existing target id for a route.
pub async fn find_target(db: &SqlitePool, route: &RouteId) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT id FROM metric_targets WHERE kind = ? AND owner_id = ?")
        .bind(kind_str(route.0))
        .bind(&route.1)
        .fetch_optional(db)
        .await
}

async fn latest_hour(db: &SqlitePool) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT MAX(ts) FROM requests_1h")
        .fetch_one(db)
        .await
}

const INSERT_COLUMNS: &str = "(target, ts, requests, s1xx, s2xx, s3xx, s4xx, s5xx, s502, s504,
     bytes_in, bytes_out, latency, upstream)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

fn clamp_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

async fn insert_row(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    target: i64,
    ts: i64,
    c: &TrafficCounters,
) -> sqlx::Result<()> {
    let sql = format!("INSERT OR REPLACE INTO {table} {INSERT_COLUMNS}");
    sqlx::query(AssertSqlSafe(sql.as_str()))
        .bind(target)
        .bind(ts)
        .bind(clamp_i64(c.requests))
        .bind(clamp_i64(c.status[0]))
        .bind(clamp_i64(c.status[1]))
        .bind(clamp_i64(c.status[2]))
        .bind(clamp_i64(c.status[3]))
        .bind(clamp_i64(c.status[4]))
        .bind(clamp_i64(c.s502))
        .bind(clamp_i64(c.s504))
        .bind(clamp_i64(c.bytes_in))
        .bind(clamp_i64(c.bytes_out))
        .bind(encode_histogram(&c.latency))
        .bind(encode_histogram(&c.upstream))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// A stored row as counters.
#[derive(sqlx::FromRow)]
struct Row {
    target: i64,
    ts: i64,
    requests: i64,
    s1xx: i64,
    s2xx: i64,
    s3xx: i64,
    s4xx: i64,
    s5xx: i64,
    s502: i64,
    s504: i64,
    bytes_in: i64,
    bytes_out: i64,
    latency: Vec<u8>,
    upstream: Vec<u8>,
}

impl Row {
    fn counters(&self) -> TrafficCounters {
        let u = |v: i64| u64::try_from(v).unwrap_or(0);
        TrafficCounters {
            requests: u(self.requests),
            status: [
                u(self.s1xx),
                u(self.s2xx),
                u(self.s3xx),
                u(self.s4xx),
                u(self.s5xx),
            ],
            s502: u(self.s502),
            s504: u(self.s504),
            bytes_in: u(self.bytes_in),
            bytes_out: u(self.bytes_out),
            latency: decode_histogram(&self.latency),
            upstream: decode_histogram(&self.upstream),
        }
    }
}

const SELECT_COLUMNS: &str = "target, ts, requests, s1xx, s2xx, s3xx, s4xx, s5xx, s502, s504,
     bytes_in, bytes_out, latency, upstream";

/// Write one minute's rows and roll [from, to) hours up, in one transaction.
pub async fn write_minute(
    db: &SqlitePool,
    minute: i64,
    rows: &[(i64, TrafficCounters)],
    rollup: Option<(i64, i64)>,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    for (target, counters) in rows {
        insert_row(&mut tx, "requests_1m", *target, minute, counters).await?;
    }
    if let Some((from, to)) = rollup {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM requests_1m
             WHERE target IN (SELECT id FROM metric_targets) AND ts >= ? AND ts < ?"
        );
        let stored: Vec<Row> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
            .bind(from)
            .bind(to)
            .fetch_all(&mut *tx)
            .await?;
        let mut hours: HashMap<(i64, i64), TrafficCounters> = HashMap::new();
        for row in &stored {
            hours
                .entry((row.target, row.ts / 3600 * 3600))
                .or_default()
                .add(&row.counters());
        }
        for ((target, hour), counters) in hours {
            insert_row(&mut tx, "requests_1h", target, hour, &counters).await?;
        }
    }
    tx.commit().await
}

// ── Queries ──

/// A supported request-metrics window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hour,
    SixHours,
    Day,
    Week,
    Month,
    Quarter,
}

impl Range {
    pub const ALLOWED: &'static str = "1h, 6h, 24h, 7d, 30d, 90d";

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "1h" => Range::Hour,
            "6h" => Range::SixHours,
            "24h" => Range::Day,
            "7d" => Range::Week,
            "30d" => Range::Month,
            "90d" => Range::Quarter,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Range::Hour => "1h",
            Range::SixHours => "6h",
            Range::Day => "24h",
            Range::Week => "7d",
            Range::Month => "30d",
            Range::Quarter => "90d",
        }
    }

    pub fn span(self) -> i64 {
        match self {
            Range::Hour => 3600,
            Range::SixHours => 6 * 3600,
            Range::Day => 86_400,
            Range::Week => 7 * 86_400,
            Range::Month => 30 * 86_400,
            Range::Quarter => 90 * 86_400,
        }
    }

    fn table(self) -> &'static str {
        match self {
            Range::Hour | Range::SixHours | Range::Day => "requests_1m",
            _ => "requests_1h",
        }
    }

    pub fn step(self) -> i64 {
        match self {
            Range::Hour | Range::SixHours => 60,
            Range::Day => 180,
            Range::Week => 3600,
            Range::Month => 7200,
            Range::Quarter => 18_000,
        }
    }

    /// First bucket start for a window ending at `now`.
    pub fn from(self, now: i64) -> i64 {
        (now - self.span()) / self.step() * self.step()
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn err_pct(status: &[u64; 5]) -> Option<f64> {
    let responses: u64 = status.iter().sum();
    (responses > 0).then(|| round2(status[4] as f64 * 100.0 / responses as f64))
}

/// Summary of summed counters over `secs` seconds.
pub fn summarize(c: &TrafficCounters, secs: i64) -> RequestSummary {
    RequestSummary {
        requests: c.requests,
        rps: round2(c.requests as f64 / secs.max(1) as f64),
        s1xx: c.status[0],
        s2xx: c.status[1],
        s3xx: c.status[2],
        s4xx: c.status[3],
        s5xx: c.status[4],
        s502: c.s502,
        s504: c.s504,
        err_pct: err_pct(&c.status),
        p50: percentile(&c.latency, 0.50),
        p95: percentile(&c.latency, 0.95),
        p99: percentile(&c.latency, 0.99),
        upstream_p50: percentile(&c.upstream, 0.50),
        upstream_p95: percentile(&c.upstream, 0.95),
        upstream_p99: percentile(&c.upstream, 0.99),
        bytes_in: c.bytes_in,
        bytes_out: c.bytes_out,
    }
}

async fn fetch_table(
    db: &SqlitePool,
    targets: Option<&[i64]>,
    table: &str,
    from: i64,
) -> sqlx::Result<Vec<Row>> {
    let filter = match targets {
        Some([]) => return Ok(Vec::new()),
        Some(t) => format!("target IN ({})", vec!["?"; t.len()].join(", ")),
        None => "target IN (SELECT id FROM metric_targets)".to_string(),
    };
    let sql = format!("SELECT {SELECT_COLUMNS} FROM {table} WHERE {filter} AND ts >= ?");
    let mut q = sqlx::query_as::<_, Row>(AssertSqlSafe(sql));
    for t in targets.unwrap_or_default() {
        q = q.bind(*t);
    }
    q.bind(from).fetch_all(db).await
}

/// Stored rows for the window: the range's table, plus — for hourly ranges —
/// the minutes not rolled up into an hour yet.
async fn fetch_rows(
    db: &SqlitePool,
    targets: Option<&[i64]>,
    range: Range,
    from: i64,
) -> sqlx::Result<Vec<Row>> {
    let mut rows = fetch_table(db, targets, range.table(), from).await?;
    if range.table() == "requests_1h" {
        let rolled_until = latest_hour(db).await?.map_or(from, |h| h + 3600);
        rows.extend(fetch_table(db, targets, "requests_1m", rolled_until.max(from)).await?);
    }
    Ok(rows)
}

/// Series (every bucket from the window start to `now`, zeros where
/// nothing was served) and summary for `targets` summed.
///
/// `extra` adds counters that are not stored yet (the pending minute).
pub async fn query(
    db: &SqlitePool,
    targets: &[i64],
    range: Range,
    now: i64,
    extra: Option<(i64, TrafficCounters)>,
) -> sqlx::Result<(i64, RequestSeries, RequestSummary)> {
    let step = range.step();
    let from = range.from(now);
    let rows = fetch_rows(db, Some(targets), range, from).await?;
    let buckets = usize::try_from((now - from) / step + 1).unwrap_or(1);
    let mut per_bucket = vec![TrafficCounters::default(); buckets];
    let mut total = TrafficCounters::default();
    let all = rows
        .iter()
        .map(|r| (r.ts, r.counters()))
        .chain(extra.filter(|(ts, _)| *ts >= from));
    for (ts, c) in all {
        if let Ok(i) = usize::try_from((ts - from) / step)
            && let Some(b) = per_bucket.get_mut(i)
        {
            b.add(&c);
        }
        total.add(&c);
    }
    let mut s = RequestSeries::default();
    let rate = |v: u64| round2(v as f64 / step as f64);
    for (i, c) in per_bucket.iter().enumerate() {
        s.t.push(from + i as i64 * step);
        s.rps.push(rate(c.requests));
        s.rps_1xx.push(rate(c.status[0]));
        s.rps_2xx.push(rate(c.status[1]));
        s.rps_3xx.push(rate(c.status[2]));
        s.rps_4xx.push(rate(c.status[3]));
        s.rps_5xx.push(rate(c.status[4]));
        s.err_pct.push(err_pct(&c.status));
        s.p50.push(percentile(&c.latency, 0.50));
        s.p95.push(percentile(&c.latency, 0.95));
        s.p99.push(percentile(&c.latency, 0.99));
        s.bytes_in.push(rate(c.bytes_in));
        s.bytes_out.push(rate(c.bytes_out));
    }
    Ok((from, s, summarize(&total, now - from)))
}

/// Summed counters per target over the window (for the traffic overview).
pub async fn totals_by_target(
    db: &SqlitePool,
    range: Range,
    now: i64,
) -> sqlx::Result<HashMap<i64, TrafficCounters>> {
    let rows = fetch_rows(db, None, range, range.from(now)).await?;
    let mut out: HashMap<i64, TrafficCounters> = HashMap::new();
    for row in &rows {
        out.entry(row.target).or_default().add(&row.counters());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use tond_common::traffic::RouteTraffic;

    use super::*;

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    fn counters(requests: u64, s5xx: u64, ms_bucket: usize) -> TrafficCounters {
        let mut c = TrafficCounters {
            requests,
            status: [0, requests - s5xx, 0, 0, s5xx],
            bytes_in: requests * 10,
            bytes_out: requests * 100,
            ..Default::default()
        };
        c.latency[ms_bucket] = requests;
        c.upstream[ms_bucket] = requests;
        c
    }

    fn snap(pid: u32, routes: &[(RouteKind, &str, u64, TrafficCounters)]) -> TrafficSnapshot {
        TrafficSnapshot {
            pid,
            routes: routes
                .iter()
                .map(|(kind, id, generation, c)| RouteTraffic {
                    kind: *kind,
                    id: (!id.is_empty()).then(|| id.to_string()),
                    generation: *generation,
                    counters: c.clone(),
                })
                .collect(),
        }
    }

    fn app(id: &str) -> RouteId {
        (RouteKind::App, id.to_string())
    }

    #[test]
    fn diffs_handle_baselines_restarts_and_recreated_routes() {
        let mut t = Tracker::default();
        // First reading: baseline only.
        let (d, present) = t.diff(snap(1, &[(RouteKind::App, "a", 5, counters(100, 0, 3))]));
        assert!(d.is_empty());
        assert_eq!(present, vec![app("a")]);

        // Normal delta; an idle route reports nothing; a new route counts fully.
        let (d, _) = t.diff(snap(
            1,
            &[
                (RouteKind::App, "a", 5, counters(130, 3, 3)),
                (RouteKind::Unknown, "", 2, counters(0, 0, 0)),
                (RouteKind::Preview, "p", 9, counters(4, 0, 1)),
            ],
        ));
        assert_eq!(d.len(), 2);
        assert_eq!(
            d[0],
            (app("a"), {
                let mut c = counters(30, 3, 3);
                c.status = [0, 27, 0, 0, 3];
                c
            })
        );
        assert_eq!(d[1].1.requests, 4);

        // Route re-created (new generation): the new counters are the delta.
        let (d, _) = t.diff(snap(1, &[(RouteKind::App, "a", 6, counters(2, 0, 3))]));
        assert_eq!(d[0].1.requests, 2);

        // Counter went backwards within a generation: treated as a reset.
        let (d, _) = t.diff(snap(1, &[(RouteKind::App, "a", 6, counters(1, 0, 3))]));
        assert_eq!(d[0].1.requests, 1);

        // Proxy restarted (new pid): everything since its start.
        let (d, _) = t.diff(snap(2, &[(RouteKind::App, "a", 6, counters(7, 0, 3))]));
        assert_eq!(d[0].1.requests, 7);
    }

    #[test]
    fn minutes_are_flushed_when_a_later_one_starts() {
        let mut t = Tracker::default();
        let d = vec![(app("a"), counters(10, 0, 3))];
        assert!(t.accumulate(60, &d).is_none());
        assert!(t.accumulate(60, &d).is_none());
        let (minute, rows) = t.accumulate(120, &d).unwrap_or_default();
        assert_eq!(minute, 60);
        assert_eq!(rows.get(&app("a")).map(|c| c.requests), Some(20));
        // Idle minutes are not written.
        let (minute, _) = t.accumulate(180, &[]).unwrap_or_default();
        assert_eq!(minute, 120);
        assert!(t.accumulate(240, &[]).is_none());
        // A late tick for an older minute joins the pending one.
        assert!(t.accumulate(200, &d).is_none());
        assert_eq!(t.pending_minute, Some(240));
    }

    #[test]
    fn ranges_stay_under_the_point_budget() {
        for r in ["1h", "6h", "24h", "7d", "30d", "90d"] {
            let range = Range::parse(r).unwrap_or(Range::Hour);
            assert_eq!(range.as_str(), r);
            assert!(range.span() / range.step() <= 480, "{r}");
        }
        assert_eq!(Range::parse("1y"), None);
    }

    #[test]
    fn live_numbers_cover_the_last_minute() {
        let route = (RouteKind::App, "live-test".to_string());
        publish_live(
            &[(route.clone(), counters(60, 6, 5))],
            std::slice::from_ref(&route),
            1000,
            30,
        );
        publish_live(
            &[(route.clone(), counters(30, 0, 5))],
            std::slice::from_ref(&route),
            1030,
            30,
        );
        let c = current(&route, 1040).unwrap_or_else(|| panic!("current"));
        assert_eq!((c.secs, c.rps), (60, 1.5));
        assert_eq!(c.err_pct, Some(6.67));
        assert!(current(&route, 2000).is_none(), "stale");
        publish_live(&[], &[], 1060, 30);
        assert!(current(&route, 1060).is_none(), "route gone");
    }

    #[tokio::test]
    async fn minutes_roll_up_into_hours_and_query_back() {
        let db = db().await;
        let a = ensure_target(&db, &app("a"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ensure_target(&db, &app("a")).await.ok(), Some(a));
        let unknown = ensure_target(&db, &(RouteKind::Unknown, String::new()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let base = 1_800_000_000 / 3600 * 3600;
        // An hour of minutes: 60 requests/min, 6 of them 5xx, ~25 ms.
        for m in 0..60 {
            write_minute(&db, base + m * 60, &[(a, counters(60, 6, 7))], None)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        }
        write_minute(
            &db,
            base + 3600,
            &[(unknown, counters(5, 0, 0))],
            Some((base, base + 3600)),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let hour: (i64, i64, i64) =
            sqlx::query_as("SELECT ts, requests, s5xx FROM requests_1h WHERE target = ?")
                .bind(a)
                .fetch_one(&db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(hour, (base, 3600, 360));

        let now = base + 3600 + 30;
        let (from, series, summary) = query(
            &db,
            &[a],
            Range::Hour,
            now,
            Some((now / 60 * 60, counters(6, 0, 7))),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(from, base);
        assert_eq!(series.t.len(), 61);
        assert_eq!(series.rps[0], 1.0);
        assert_eq!(series.rps_5xx[0], 0.1);
        assert_eq!(series.err_pct[0], Some(10.0));
        assert_eq!(series.p50[0], Some(25.0));
        assert_eq!(series.rps[60], 0.1, "the pending minute is included");
        assert_eq!(summary.requests, 3606);
        assert_eq!(summary.s5xx, 360);
        assert_eq!(summary.err_pct, Some(9.98));
        assert_eq!(summary.bytes_out, 360_600);
        assert!(summary.upstream_p95.is_some());

        // 7d reads the hourly table.
        let (_, week, summary) = query(&db, &[a], Range::Week, now, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(summary.requests, 3600);
        assert_eq!(week.rps.iter().filter(|v| **v > 0.0).count(), 1);
        // Minutes of the hour that is not rolled up yet count on hourly ranges.
        let (_, _, recent) = query(&db, &[unknown], Range::Week, now, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(recent.requests, 5);

        let totals = totals_by_target(&db, Range::Day, now)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(totals.get(&a).map(|c| c.requests), Some(3600));
        assert_eq!(totals.get(&unknown).map(|c| c.requests), Some(5));
        assert_eq!(find_target(&db, &app("a")).await.ok().flatten(), Some(a));
        assert_eq!(find_target(&db, &app("zz")).await.ok().flatten(), None);
    }
}
