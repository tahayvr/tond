-- 017_audit_log.sql — Phase 5.5: Audit log table
--
-- Stores a record of every state-changing operation performed through
-- the API. Each row captures:
--   app_id    — the target app (NULL for platform-level actions)
--   app_name  — denormalized snapshot; avoids JOIN when app is deleted
--   action    — short verb (e.g. "create", "deploy", "stop", "env_update")
--   actor     — email of the session user, or "api-key" for bearer auth
--   detail    — optional JSON string with additional context
--   created_at — UTC timestamp of the operation

CREATE TABLE IF NOT EXISTS audit_log (
    id         TEXT PRIMARY KEY,
    app_id     TEXT,
    app_name   TEXT,
    action     TEXT NOT NULL,
    actor      TEXT NOT NULL,
    detail     TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_audit_log_app_id    ON audit_log (app_id);
CREATE INDEX IF NOT EXISTS idx_audit_log_created_at ON audit_log (created_at DESC);
