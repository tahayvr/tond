// runtime/dbstats.rs — count every SQL statement tond-server runs
//
// sqlx emits one tracing event per executed statement (target "sqlx::query",
// DEBUG; WARN when slower than the slow-statement threshold configured in
// db::open_pool). DbStatsLayer is a tracing layer with its own per-layer
// filter that sees only those events and keeps:
//
//   - a total statement counter and a slow-statement counter,
//   - the last few slow statements (SQL, duration, time),
//   - a bounded map of distinct statements -> execution count, so the idle
//     DB-queries budget can be checked (and EXPLAINed) by e2e/budget.sh.
//
// The journal is not involved: the fmt layer keeps its own EnvFilter, so
// statements are never printed at the default RUST_LOG=info.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use tond_common::{SlowStatement, StatementCount};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Statements slower than this are logged as WARN and counted as slow.
pub const SLOW_THRESHOLD_MS: u64 = 100;
/// Distinct statements tracked.
const MAX_DISTINCT: usize = 256;
/// Slow statements remembered.
const MAX_RECENT_SLOW: usize = 10;
/// Longest SQL text kept per statement.
const MAX_SQL_LEN: usize = 2_000;

static QUERIES: AtomicU64 = AtomicU64::new(0);
static SLOW: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Detail {
    counts: HashMap<String, u64>,
    recent_slow: VecDeque<SlowStatement>,
}

static DETAIL: LazyLock<Mutex<Detail>> = LazyLock::new(|| Mutex::new(Detail::default()));

pub fn queries_total() -> u64 {
    QUERIES.load(Ordering::Relaxed)
}

pub fn slow_total() -> u64 {
    SLOW.load(Ordering::Relaxed)
}

/// The most recent slow statements, newest first.
pub fn recent_slow() -> Vec<SlowStatement> {
    let d = DETAIL.lock().unwrap_or_else(|e| e.into_inner());
    d.recent_slow.iter().rev().cloned().collect()
}

/// The `limit` most executed statements, most frequent first.
pub fn top_statements(limit: usize) -> Vec<StatementCount> {
    let d = DETAIL.lock().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<StatementCount> = d
        .counts
        .iter()
        .map(|(sql, count)| StatementCount {
            sql: sql.clone(),
            count: *count,
        })
        .collect();
    all.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.sql.cmp(&b.sql)));
    all.truncate(limit);
    all
}

#[derive(Default)]
struct QueryFields {
    summary: String,
    statement: String,
    elapsed_secs: f64,
}

impl Visit for QueryFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" => self.summary = value.to_string(),
            "db.statement" => self.statement = value.to_string(),
            _ => {}
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed_secs = value;
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `summary` and `db.statement` arrive as Strings via record_debug on
        // some tracing versions.
        match field.name() {
            "summary" if self.summary.is_empty() => {
                self.summary = format!("{value:?}").trim_matches('"').to_string()
            }
            "db.statement" if self.statement.is_empty() => {
                self.statement = format!("{value:?}").trim_matches('"').to_string()
            }
            _ => {}
        }
    }
}

impl QueryFields {
    /// sqlx puts the full SQL in `db.statement` only when the summary had to
    /// abbreviate it; collapse whitespace so identical statements match.
    fn sql(&self) -> String {
        let raw = if self.statement.trim().is_empty() {
            self.summary.as_str()
        } else {
            self.statement.as_str()
        };
        let mut sql = raw
            .replace("\\n", " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if sql.len() > MAX_SQL_LEN {
            let mut end = MAX_SQL_LEN;
            while !sql.is_char_boundary(end) {
                end -= 1;
            }
            sql.truncate(end);
        }
        sql
    }
}

fn record(fields: &QueryFields, slow: bool) {
    QUERIES.fetch_add(1, Ordering::Relaxed);
    let sql = fields.sql();
    let mut d = DETAIL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = d.counts.get_mut(&sql) {
        *n += 1;
    } else if d.counts.len() < MAX_DISTINCT {
        d.counts.insert(sql.clone(), 1);
    }
    if slow {
        SLOW.fetch_add(1, Ordering::Relaxed);
        if d.recent_slow.len() == MAX_RECENT_SLOW {
            d.recent_slow.pop_front();
        }
        d.recent_slow.push_back(SlowStatement {
            sql,
            elapsed_ms: fields.elapsed_secs * 1000.0,
            at: chrono::Utc::now().to_rfc3339(),
        });
    }
}

/// Tracing layer that feeds the counters. Install with a per-layer filter
/// that passes `sqlx::query` at DEBUG (see `filter`).
pub struct DbStatsLayer;

impl DbStatsLayer {
    /// The per-layer filter: only sqlx statement events.
    pub fn filter() -> tracing_subscriber::filter::Targets {
        tracing_subscriber::filter::Targets::new().with_target("sqlx::query", Level::DEBUG)
    }
}

impl<S: Subscriber> Layer<S> for DbStatsLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut fields = QueryFields::default();
        event.record(&mut fields);
        record(&fields, *event.metadata().level() <= Level::WARN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_statement_wins_over_summary_and_whitespace_is_collapsed() {
        let f = QueryFields {
            summary: "select id from …".into(),
            statement: "\n\nSELECT id\n   FROM apps WHERE x = ?\n".into(),
            elapsed_secs: 0.0,
        };
        assert_eq!(f.sql(), "SELECT id FROM apps WHERE x = ?");
        let g = QueryFields {
            summary: "PRAGMA optimize".into(),
            ..Default::default()
        };
        assert_eq!(g.sql(), "PRAGMA optimize");
    }

    #[test]
    fn layer_counts_sqlx_statement_events_and_ignores_other_targets() {
        use tracing_subscriber::prelude::*;
        let subscriber =
            tracing_subscriber::registry().with(DbStatsLayer.with_filter(DbStatsLayer::filter()));
        let _guard = tracing::subscriber::set_default(subscriber);

        let before = queries_total();
        // The same shape sqlx's QueryLogger emits (sqlx runs SQLite on its own
        // worker thread, so a thread-local subscriber can't see real queries).
        tracing::event!(
            target: "sqlx::query",
            Level::DEBUG,
            summary = "SELECT 424242",
            db.statement = "",
            rows_affected = 0u64,
            rows_returned = 1u64,
            elapsed_secs = 0.001f64,
        );
        tracing::event!(target: "tond_server::other", Level::WARN, summary = "SELECT 1");
        assert_eq!(queries_total(), before + 1);
        assert!(
            top_statements(MAX_DISTINCT)
                .iter()
                .any(|s| s.sql == "SELECT 424242"),
            "the statement is tracked by its SQL"
        );

        record(
            &QueryFields {
                summary: "SELECT slow".into(),
                elapsed_secs: 0.25,
                ..Default::default()
            },
            true,
        );
        assert!(slow_total() >= 1);
        let recent = recent_slow();
        assert_eq!(recent.first().map(|s| s.sql.as_str()), Some("SELECT slow"));
        assert_eq!(recent.first().map(|s| s.elapsed_ms), Some(250.0));
    }
}
