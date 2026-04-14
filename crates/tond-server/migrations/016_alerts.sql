-- 016_alerts.sql
-- Configurable alert rules for monitoring apps and host resources.
-- When a condition fires, a POST is sent to the configured webhook_url.
CREATE TABLE IF NOT EXISTS alert_rules (
    id           TEXT    NOT NULL PRIMARY KEY,
    -- Optional: scope to a specific app; NULL = all apps
    app_id       TEXT    REFERENCES apps(id) ON DELETE CASCADE,
    -- Human-readable name
    name         TEXT    NOT NULL,
    -- Metric to watch: "cpu" | "memory" | "app_down"
    metric       TEXT    NOT NULL,
    -- Threshold:
    --   cpu / memory → percentage (0–100)
    --   app_down     → minutes the app must be non-running before alerting
    threshold    REAL    NOT NULL,
    -- Webhook URL to POST to when the condition fires
    webhook_url  TEXT    NOT NULL,
    -- 1 = active, 0 = paused
    enabled      INTEGER NOT NULL DEFAULT 1,
    -- ISO-8601 timestamp of last fire (NULL = never fired)
    last_fired_at TEXT,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);
