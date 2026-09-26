-- Compact resource metrics with rollups (replaces the `metrics` table).
--
-- The old table stored ~300 B per sample (TEXT uuid, TEXT timestamp, REAL
-- cpu, cumulative network counters) and was pruned with a full scan. The new
-- layout keeps one small row per target per tick, keyed by (target, ts), in
-- WITHOUT ROWID tables so the primary key IS the row (~30-40 B per row):
--
--   metric_targets  one row per measured thing (app, database, cron job,
--                   preview), mapped to a small integer id.
--   metrics_raw     30 s samples, kept 25 h.
--   metrics_5m      5-minute rollups, kept 8 days.
--   metrics_1h      1-hour rollups, kept 90 days.
--   metric_events   restarts, crashes and OOM kills, kept 90 days.
--
-- Units: ts is unix seconds; cpu is hundredths of a percent of one core
-- (10000 = one full core); mem is bytes; rx / tx are bytes per second
-- (rates, not counters); pids is a process count. Retention runs in the
-- janitor (janitor.rs → metrics::store::prune).

DROP INDEX IF EXISTS idx_metrics_app_collected;
DROP INDEX IF EXISTS idx_metrics_collected_at;
DROP TABLE IF EXISTS metrics;

CREATE TABLE metric_targets(
  id INTEGER PRIMARY KEY,
  -- "app" | "database" | "cron" | "preview"
  kind TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  -- Latest summed memory limit of the target's containers in bytes (0 = none)
  mem_limit INTEGER NOT NULL DEFAULT 0,
  UNIQUE(kind, owner_id)
);

CREATE TABLE metrics_raw(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

-- Rollups: cpu / mem / rx / tx are bucket averages, *_max the bucket maxima.
CREATE TABLE metrics_5m(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  mem_max INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE metrics_1h(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  mem_max INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE metric_events(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  -- "restart" | "oom" | "crash"
  kind TEXT NOT NULL,
  detail TEXT,
  PRIMARY KEY(target, ts, kind)
) WITHOUT ROWID;
