-- Event-only alert rules ("notify me when a deploy fails") have no metric, but
-- the baseline declared alert_rules.metric NOT NULL, so creating one failed.
-- SQLite can't drop a NOT NULL constraint in place: rebuild the table, keeping
-- the rule ↔ channel links (dropping alert_rules cascades to them).

CREATE TEMP TABLE alert_rule_notifications_keep AS SELECT * FROM alert_rule_notifications;

CREATE TABLE alert_rules_new(
  id TEXT NOT NULL PRIMARY KEY,
  -- Optional: scope to a specific app; NULL = all apps
  app_id TEXT REFERENCES apps(id) ON DELETE CASCADE,
  -- Human-readable name
  name TEXT NOT NULL,
  -- Metric to watch: "cpu" | "memory" | "app_down" | "disk_usage"; NULL for event-only rules
  metric TEXT,
  -- Threshold:
  -- cpu / memory / disk_usage → percentage(0–100)
  -- app_down → minutes the app must be non-running before alerting
  threshold REAL NOT NULL,
  -- Optional webhook URL to POST to when the rule fires ('' = none)
  webhook_url TEXT NOT NULL DEFAULT '',
  -- 1 = active, 0 = paused
  enabled INTEGER NOT NULL DEFAULT 1,
  -- ISO-8601 timestamp of last fire(NULL = never fired)
  last_fired_at TEXT,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  events TEXT
);

INSERT INTO alert_rules_new (id, app_id, name, metric, threshold, webhook_url, enabled, last_fired_at, created_at, updated_at, events)
SELECT id, app_id, name, metric, threshold, webhook_url, enabled, last_fired_at, created_at, updated_at, events
FROM alert_rules;

DROP TABLE alert_rules;
ALTER TABLE alert_rules_new RENAME TO alert_rules;

DELETE FROM alert_rule_notifications;
INSERT INTO alert_rule_notifications SELECT * FROM alert_rule_notifications_keep;
DROP TABLE alert_rule_notifications_keep;
