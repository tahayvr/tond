-- 042_project_previews.sql
-- Project-level preview environments.
-- A project preview groups one preview deployment per member app under a
-- shared slug so their URLs are visually related:
--   e.g. abc123-frontend.domain.com, abc123-api.domain.com
--
-- project_previews is the "envelope": one row per project preview session.
-- preview_deployments gains a nullable project_preview_id FK so all existing
-- per-app previews keep working unchanged.

CREATE TABLE IF NOT EXISTS project_previews (
    id          TEXT PRIMARY KEY NOT NULL,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Shared slug used as the prefix for every member app's subdomain
    slug        TEXT NOT NULL,
    -- TTL in seconds; mirrors the ttl_seconds on child preview_deployments
    ttl_seconds INTEGER NOT NULL DEFAULT 86400,
    expires_at  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Link each preview_deployment back to its project preview (nullable so
-- stand-alone per-app previews are unaffected).
ALTER TABLE preview_deployments ADD COLUMN project_preview_id TEXT REFERENCES project_previews(id) ON DELETE SET NULL;
