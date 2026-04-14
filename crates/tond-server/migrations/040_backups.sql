-- 040_backups.sql
-- Creates the `snapshots` and `backup_config` tables.
--
-- `snapshots` records every backup taken: scheduled, manual, pre-deploy, and
-- pre-restore safety nets. Hard-link deduplication means `delta_bytes` (physical
-- bytes written) is usually much smaller than `total_bytes` (logical size).
--
-- `backup_config` is a single-row settings table (id must equal 1).
-- The row is seeded with defaults on first migration run.

CREATE TABLE IF NOT EXISTS snapshots (
    id                   TEXT NOT NULL PRIMARY KEY,
    name                 TEXT NOT NULL,
    -- "pending" | "running" | "completed" | "failed"
    status               TEXT NOT NULL DEFAULT 'pending',
    -- "scheduled" | "manual" | "pre_deploy" | "pre_restore"
    trigger              TEXT NOT NULL DEFAULT 'manual',
    -- "full" | "app:{name}" | "database:{name}"
    scope                TEXT NOT NULL DEFAULT 'full',
    -- set when trigger = "pre_deploy"
    trigger_app          TEXT,
    deployment_id        TEXT REFERENCES deployments(id) ON DELETE SET NULL,
    -- previous snapshot used as hard-link base
    parent_snapshot_id   TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at         TEXT,
    -- bytes physically written (not counting hard-linked files)
    delta_bytes          INTEGER,
    -- total logical size if all hard links were resolved
    total_bytes          INTEGER,
    -- absolute path to local snapshot directory
    local_path           TEXT,
    -- object key on the remote storage provider (if uploaded)
    remote_key           TEXT,
    storage_provider_id  TEXT REFERENCES storage_providers(id) ON DELETE SET NULL,
    -- SHA-256 of the exported .tar.gz (if uploaded)
    checksum_sha256      TEXT,
    error_message        TEXT,
    notes                TEXT
);

CREATE INDEX IF NOT EXISTS snapshots_trigger_app ON snapshots(trigger_app, created_at DESC);
CREATE INDEX IF NOT EXISTS snapshots_created_at  ON snapshots(created_at DESC);

CREATE TABLE IF NOT EXISTS backup_config (
    id                           INTEGER PRIMARY KEY CHECK (id = 1),
    enabled                      INTEGER NOT NULL DEFAULT 0,
    full_schedule_cron           TEXT    NOT NULL DEFAULT '0 3 * * *',
    full_retention_count         INTEGER NOT NULL DEFAULT 7,
    pre_deploy_enabled           INTEGER NOT NULL DEFAULT 1,
    pre_deploy_retention_per_app INTEGER NOT NULL DEFAULT 10,
    keep_local                   INTEGER NOT NULL DEFAULT 1,
    storage_provider_id          TEXT REFERENCES storage_providers(id) ON DELETE SET NULL
);

-- Seed the single config row with defaults on first run.
INSERT OR IGNORE INTO backup_config (id) VALUES (1);
