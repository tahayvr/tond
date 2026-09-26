-- Phase 2 · Observable: host history, uptime checks, outbound heartbeat.
--
-- Everything here is written by tond-server only. Time series use integer
-- unix timestamps and WITHOUT ROWID tables keyed by (series, ts), like the
-- container metrics tables.

-- ── Host history (metrics/host_history.rs) ──────────────────────────────────
-- One row per minute (kept 48 h) and per hour (kept 1 year). Percentages and
-- load averages are stored × 100 as integers; sizes in bytes. `disk_*` and
-- `inodes_*` describe the filesystem holding the Tond data directory,
-- `root_*` the filesystem mounted at / (the same numbers when they are one
-- filesystem).
CREATE TABLE host_1m(
  ts INTEGER PRIMARY KEY NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  load1 INTEGER NOT NULL,
  load5 INTEGER NOT NULL,
  load15 INTEGER NOT NULL,
  mem_used INTEGER NOT NULL,
  mem_total INTEGER NOT NULL,
  swap_used INTEGER NOT NULL,
  swap_total INTEGER NOT NULL,
  disk_used INTEGER NOT NULL,
  disk_total INTEGER NOT NULL,
  inodes_used INTEGER NOT NULL,
  inodes_total INTEGER NOT NULL,
  root_used INTEGER NOT NULL,
  root_total INTEGER NOT NULL,
  root_inodes_used INTEGER NOT NULL,
  root_inodes_total INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE host_1h(
  ts INTEGER PRIMARY KEY NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  load1 INTEGER NOT NULL,
  load5 INTEGER NOT NULL,
  load15 INTEGER NOT NULL,
  mem_used INTEGER NOT NULL,
  mem_total INTEGER NOT NULL,
  swap_used INTEGER NOT NULL,
  swap_total INTEGER NOT NULL,
  disk_used INTEGER NOT NULL,
  disk_total INTEGER NOT NULL,
  inodes_used INTEGER NOT NULL,
  inodes_total INTEGER NOT NULL,
  root_used INTEGER NOT NULL,
  root_total INTEGER NOT NULL,
  root_inodes_used INTEGER NOT NULL,
  root_inodes_total INTEGER NOT NULL
) WITHOUT ROWID;

-- ── Uptime checks (uptime/) ─────────────────────────────────────────────────
-- One check per app with a domain (created automatically) and one for the
-- dashboard. Deleting an app deletes its check and the check's history.
CREATE TABLE uptime_checks(
  id INTEGER PRIMARY KEY,
  -- "app" | "dashboard"
  kind TEXT NOT NULL,
  -- apps.id for "app"; '' for the dashboard
  owner_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  path TEXT NOT NULL DEFAULT '/',
  -- seconds between probes: a multiple of 60, 60 – 3600
  interval_secs INTEGER NOT NULL DEFAULT 60,
  -- expected HTTP status; NULL = any 2xx or 3xx
  expected_status INTEGER,
  timeout_secs INTEGER NOT NULL DEFAULT 10,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  UNIQUE(kind, owner_id)
);

-- One row per probe (probes are minute-aligned), kept 48 h.
--   result: 0 up, 1 dns, 2 refused, 3 connect, 4 timeout, 5 tls,
--           6 http_5xx, 7 http_status (unexpected non-5xx status)
CREATE TABLE uptime_1m(
  check_id INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  result INTEGER NOT NULL,
  status INTEGER,
  latency_ms INTEGER,
  PRIMARY KEY(check_id, ts)
) WITHOUT ROWID;

-- Hourly rollups, kept 90 days. latency_ms: average of the successful probes.
CREATE TABLE uptime_1h(
  check_id INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  probes INTEGER NOT NULL,
  up INTEGER NOT NULL,
  latency_ms INTEGER,
  PRIMARY KEY(check_id, ts)
) WITHOUT ROWID;

-- Down periods, kept 90 days after they end. This is also the durable event
-- stream for alerting: a new row is a "down" event, setting ended_at is the
-- matching "up" event (see uptime/mod.rs).
--   cause: "dns" | "refused" | "connect" | "timeout" | "tls" | "http_5xx" | "http_status"
CREATE TABLE uptime_incidents(
  id INTEGER PRIMARY KEY,
  check_id INTEGER NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  cause TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_uptime_incidents_check ON uptime_incidents(check_id, started_at);
CREATE INDEX idx_uptime_incidents_started ON uptime_incidents(started_at);

CREATE TRIGGER uptime_checks_app_delete AFTER DELETE ON apps
BEGIN
  DELETE FROM uptime_checks WHERE kind = 'app' AND owner_id = OLD.id;
END;
CREATE TRIGGER uptime_checks_delete AFTER DELETE ON uptime_checks
BEGIN
  DELETE FROM uptime_1m WHERE check_id = OLD.id;
  DELETE FROM uptime_1h WHERE check_id = OLD.id;
  DELETE FROM uptime_incidents WHERE check_id = OLD.id;
END;

INSERT INTO uptime_checks (kind, owner_id, path) VALUES ('dashboard', '', '/health');

-- ── Outbound heartbeat (uptime/heartbeat.rs) ────────────────────────────────
-- A single row. url_enc is encrypted (secret_key::Purpose::HeartbeatUrl);
-- '' = not configured.
CREATE TABLE heartbeat_config(
  id TEXT PRIMARY KEY NOT NULL CHECK(id = 'heartbeat'),
  url_enc TEXT NOT NULL DEFAULT '',
  -- "GET" | "POST"
  method TEXT NOT NULL DEFAULT 'GET',
  interval_secs INTEGER NOT NULL DEFAULT 300,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_ok_at INTEGER,
  last_failure_at INTEGER,
  last_error TEXT,
  -- why the last ping was not sent (Tond itself unhealthy), when it was not
  last_skipped_at INTEGER,
  last_skip_reason TEXT
);
INSERT INTO heartbeat_config (id) VALUES ('heartbeat');
