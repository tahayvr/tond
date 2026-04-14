-- 037_alert_events.sql
--
-- Adds two columns to alert_rules:
--
--   events TEXT  — JSON array of event slugs this rule subscribes to.
--                  Valid values:
--                    "deployment_success" | "deployment_failure"
--                    "cron_success"       | "cron_failure"
--                  NULL or '[]' means no event subscriptions.
--                  A rule may have both a metric AND events.
--
--   metric is extended to also accept "disk_usage" (threshold = percentage).
--
-- No data migration needed — existing rules default to NULL (no events).

ALTER TABLE alert_rules ADD COLUMN events TEXT;
