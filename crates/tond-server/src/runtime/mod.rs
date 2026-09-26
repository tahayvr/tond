// runtime/ — how tond-server runs itself, and how it measures itself
//
//   changes      — "table X changed" notifications fed by SQLite hooks; lets
//                  background loops sleep until there is work instead of polling
//   loops        — registry of background loops: wakeups, runs, errors, lag
//   shutdown     — SIGTERM handling and tracking of in-flight work
//   dbstats      — SQL statement counters (tracing layer on sqlx's events)
//   http_metrics — per-route latency histograms (axum route layer)
//   selfstats    — GET /api/system/self and the Prometheus /metrics text

pub mod changes;
pub mod dbstats;
pub mod http_metrics;
pub mod loops;
pub mod selfstats;
pub mod shutdown;
