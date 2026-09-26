-- Request metrics (tond-proxy's per-route counters, scraped by tond-server)
-- and opt-in per-app access logs.

-- Per-minute totals of one target (metric_targets: kind 'app' | 'preview',
-- or 'dashboard' / 'unknown' with owner_id ''), written once a minute for
-- targets that served requests. Kept 48 hours.
CREATE TABLE requests_1m(
  target INTEGER NOT NULL,
  -- Minute start, unix seconds.
  ts INTEGER NOT NULL,
  requests INTEGER NOT NULL,
  s1xx INTEGER NOT NULL,
  s2xx INTEGER NOT NULL,
  s3xx INTEGER NOT NULL,
  s4xx INTEGER NOT NULL,
  s5xx INTEGER NOT NULL,
  s502 INTEGER NOT NULL,
  s504 INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  -- Latency histograms (tond_common::traffic::encode_histogram): total time
  -- and time to the upstream's response head.
  latency BLOB NOT NULL,
  upstream BLOB NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

-- Hourly rollups of requests_1m (same columns). Kept 90 days.
CREATE TABLE requests_1h(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  requests INTEGER NOT NULL,
  s1xx INTEGER NOT NULL,
  s2xx INTEGER NOT NULL,
  s3xx INTEGER NOT NULL,
  s4xx INTEGER NOT NULL,
  s5xx INTEGER NOT NULL,
  s502 INTEGER NOT NULL,
  s504 INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  latency BLOB NOT NULL,
  upstream BLOB NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

-- Opt-in access logs (tond-proxy appends to <data>/access/<app id>.log).
-- access_log_query: also write the query string (off by default: query
-- strings often carry tokens).
ALTER TABLE apps ADD COLUMN access_log INTEGER NOT NULL DEFAULT 0;
ALTER TABLE apps ADD COLUMN access_log_query INTEGER NOT NULL DEFAULT 0;

-- The proxy reads both on route reloads.
CREATE TRIGGER route_apps_access_log AFTER UPDATE OF access_log, access_log_query ON apps
WHEN OLD.access_log IS NOT NEW.access_log
  OR OLD.access_log_query IS NOT NEW.access_log_query
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
