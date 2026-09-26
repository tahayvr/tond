-- Alert engine: one condition per rule with a `for` duration, severity and
-- repeat interval; per-(rule, target) state; alert history (one row per
-- firing episode); a delivery queue with retries; channel quiet hours and
-- delivery health. Pre-release: the old event/threshold rules are replaced.

DROP TABLE alert_rule_notifications;
DROP TABLE alert_rules;

CREATE TABLE alert_rules(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- tond_common::ALERT_KINDS: cpu | memory | app_down | disk_usage |
  -- disk_forecast | http_5xx_rate | http_p95_latency | cert_expiry | oom |
  -- crash_loop | backup_failed | deploy_failed | cron_failed
  kind TEXT NOT NULL,
  -- App-scoped kinds: one app, or NULL for every app. Ignored otherwise.
  app_id TEXT REFERENCES apps(id) ON DELETE CASCADE,
  -- Unit depends on the kind (percent, ms, days, hours, restarts)
  threshold REAL NOT NULL DEFAULT 0,
  -- Look-back window (5xx rate, p95 latency, OOM, crash loop), seconds
  window_secs INTEGER NOT NULL DEFAULT 0,
  -- How long the condition must hold before the alert fires, seconds
  for_secs INTEGER NOT NULL DEFAULT 0,
  -- info | warning | critical
  severity TEXT NOT NULL DEFAULT 'warning',
  -- Re-notify while firing and unacknowledged; 0 = never
  repeat_secs INTEGER NOT NULL DEFAULT 14400,
  enabled INTEGER NOT NULL DEFAULT 1,
  -- The whole rule is muted until this unix time (NULL = not muted)
  snoozed_until INTEGER,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE alert_rule_notifications(
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, integration_id)
) WITHOUT ROWID;

-- One row per firing episode (fired → resolved). Kept 90 days after it
-- resolved (janitor). Rule fields are copied so history survives edits and
-- deletes.
CREATE TABLE alert_history(
  id TEXT NOT NULL PRIMARY KEY,
  rule_id TEXT REFERENCES alert_rules(id) ON DELETE SET NULL,
  rule_name TEXT NOT NULL,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  -- "app:<id>" | "host" | "cert:<domain>" | "backup" | "cron:<id>"
  target TEXT NOT NULL,
  target_label TEXT NOT NULL,
  app_id TEXT,
  threshold REAL NOT NULL,
  -- Value when it fired, and the latest value seen while firing
  value REAL,
  last_value REAL,
  -- Unix seconds: condition first true, fired, resolved
  pending_since INTEGER NOT NULL,
  fired_at INTEGER NOT NULL,
  resolved_at INTEGER,
  acknowledged_at INTEGER,
  acknowledged_by TEXT,
  last_notified_at INTEGER
);
CREATE INDEX alert_history_fired ON alert_history(fired_at DESC);
CREATE INDEX alert_history_rule ON alert_history(rule_id, fired_at DESC);
CREATE INDEX alert_history_app ON alert_history(app_id, fired_at DESC);
CREATE INDEX alert_history_resolved ON alert_history(resolved_at);

-- Live state per (rule, target). No row = inactive. A row in state
-- 'inactive' only carries a timed snooze that outlives its episode;
-- snoozed_until = ALERT_SNOOZE_UNTIL_RESOLVED mutes until it resolves.
CREATE TABLE alert_states(
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  target TEXT NOT NULL,
  target_label TEXT NOT NULL,
  -- pending | firing | inactive
  state TEXT NOT NULL,
  -- Unix seconds the condition became true
  since INTEGER NOT NULL,
  value REAL,
  episode_id TEXT REFERENCES alert_history(id) ON DELETE SET NULL,
  -- Timed snooze of this target (unix seconds)
  snoozed_until INTEGER,
  PRIMARY KEY(rule_id, target)
) WITHOUT ROWID;

-- Notifications to send (and sent). Retried with backoff; failures stay
-- visible on the alert and on the channel.
CREATE TABLE alert_deliveries(
  id TEXT NOT NULL PRIMARY KEY,
  episode_id TEXT NOT NULL REFERENCES alert_history(id) ON DELETE CASCADE,
  integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
  -- firing | repeat | resolved
  event TEXT NOT NULL,
  -- pending | sent | failed | skipped
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  -- 1 when held back by quiet hours
  deferred INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  sent_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX alert_deliveries_due ON alert_deliveries(status, next_attempt_at);
CREATE INDEX alert_deliveries_episode ON alert_deliveries(episode_id);

-- Channel quiet hours (JSON, see tond_common::ChannelQuietHours; NULL =
-- follow the global setting `alert_quiet_hours`) and delivery health.
ALTER TABLE integrations ADD COLUMN quiet_hours TEXT;
ALTER TABLE integrations ADD COLUMN last_sent_at INTEGER;
ALTER TABLE integrations ADD COLUMN last_error TEXT;
ALTER TABLE integrations ADD COLUMN last_error_at INTEGER;
