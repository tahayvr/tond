-- 019_databases.sql — Managed Databases (Phase 6.3)
--
-- Stores metadata for managed database containers (postgres, mysql, redis)
-- that Tond creates and manages on behalf of the user. Each database runs
-- as a named Docker container with a persistent host-mounted volume and an
-- auto-assigned host port for external access.
--
-- db_type:    "postgres" | "mysql" | "redis"
-- status:     "pending" | "running" | "stopped" | "failed"
-- project_id: optional — when set, the container is also connected to the
--             tond-{project_id} Docker bridge network with `name` as alias.

CREATE TABLE IF NOT EXISTS databases (
    id            TEXT PRIMARY KEY NOT NULL,
    name          TEXT NOT NULL UNIQUE,
    db_type       TEXT NOT NULL,
    project_id    TEXT REFERENCES projects(id) ON DELETE SET NULL,
    container_id  TEXT,
    host_port     INTEGER,
    status        TEXT NOT NULL DEFAULT 'pending',
    db_name       TEXT,
    db_user       TEXT,
    db_password   TEXT,
    volume_path   TEXT,
    memory_limit  INTEGER,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
