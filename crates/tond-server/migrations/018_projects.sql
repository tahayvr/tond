-- 018_projects.sql — App Groups / Projects (Phase 6.1)
--
-- A project groups related apps (e.g. frontend + backend + worker) under a
-- single logical unit. Projects also carry shared environment variables that
-- are merged into every member app's environment at deploy time.
--
-- apps.project_id is a nullable FK so existing apps keep working without a
-- project association.

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    env_vars    TEXT NOT NULL DEFAULT '{}',  -- JSON object of shared env vars
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE apps ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;
