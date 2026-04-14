-- 005_settings.sql
-- Key-value store for server-wide configuration.
-- Each row is a single setting identified by its key.
-- Values are stored as TEXT (JSON-serialisable strings).
-- seeded_at tracks when the row was first inserted so we can
-- distinguish "never set" from "explicitly set to empty".

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Seed default rows so GET /api/settings always returns a full object
-- even on a brand-new install.
INSERT OR IGNORE INTO settings (key, value) VALUES ('dashboard_domain', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('acme_email', '');
