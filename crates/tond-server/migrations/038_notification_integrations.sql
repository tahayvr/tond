-- 038_notification_integrations.sql
-- Creates the `integrations` table for named notification channels.
-- Each row stores a provider kind and a JSON config blob.
-- The `alert_rule_notifications` join table (created in 036) references
-- rows here by `integration_id`.

CREATE TABLE IF NOT EXISTS integrations (
    id         TEXT NOT NULL PRIMARY KEY,
    name       TEXT NOT NULL,
    -- "telegram" | "discord" | "slack" | "email"
    kind       TEXT NOT NULL,
    -- Provider-specific JSON config (see notifications.rs for shapes)
    config     TEXT NOT NULL DEFAULT '{}',
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
