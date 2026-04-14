-- 014_preview_deployments.sql
-- Preview deployments: ephemeral containers with a unique subdomain and TTL.
-- Each row represents one live preview created via `tond deploy <app> --preview`
-- or POST /api/apps/{id}/preview. The proxy reads this table alongside `apps`
-- when refreshing its route map so the preview subdomain is served immediately.

CREATE TABLE IF NOT EXISTS preview_deployments (
    id            TEXT PRIMARY KEY NOT NULL,
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    -- Short random hex prefix used to build the subdomain
    -- e.g. "a1b2c3" -> "a1b2c3-myapp.yourdomain.com"
    slug          TEXT NOT NULL,
    -- Full subdomain assigned to this preview (slug + "-" + app_name + "." + dashboard_domain)
    subdomain     TEXT,
    container_id  TEXT,
    host_port     INTEGER,
    image_tag     TEXT,
    -- TTL in seconds from created_at; background cleaner uses this to purge old rows
    ttl_seconds   INTEGER NOT NULL DEFAULT 86400,
    -- "building" | "running" | "failed" | "expired"
    status        TEXT NOT NULL DEFAULT 'building',
    expires_at    TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
