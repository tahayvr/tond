CREATE TABLE IF NOT EXISTS alert_rule_notifications (
    id             TEXT NOT NULL PRIMARY KEY,
    rule_id        TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
    integration_id TEXT NOT NULL,
    created_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
