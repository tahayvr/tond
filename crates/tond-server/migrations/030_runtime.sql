-- Phase 1 runtime: build queue, route_version for the event-driven proxy.

-- ── Build/deploy queue ────────────────────────────────────────────────────
-- deployments.status is now:
--   'queued'     waiting for the app's previous deploy or a build slot
--   'running'    in progress (the phases column says which step)
--   'succeeded' | 'failed'
--   'superseded' a newer deploy of the same app replaced it while queued
-- kind: 'deploy' (build or pull from the app's source), 'rollback' (re-run
-- image_tag), 'upload' (build the uploaded tarball).
ALTER TABLE deployments ADD COLUMN kind TEXT NOT NULL DEFAULT 'deploy';
-- Why a deployment failed or was superseded/interrupted (for the dashboard).
ALTER TABLE deployments ADD COLUMN error TEXT;
-- The deployment that replaced this one (status 'superseded').
ALTER TABLE deployments ADD COLUMN superseded_by TEXT;
-- Startup reconciliation and the queue look up unfinished rows.
CREATE INDEX deployments_unfinished_idx ON deployments(status, created_at)
  WHERE status IN ('queued', 'running');

-- ── Route version ─────────────────────────────────────────────────────────
-- A single counter bumped whenever anything tond-proxy routes on changes.
-- tond-proxy reloads its routing table only when the counter moves, and
-- tond-server (which sees its own writes to this table through an SQLite
-- update hook) nudges the proxy over its control socket and waits for the
-- acknowledgement. The triggers keep the counter correct no matter which
-- code path writes.
-- Folding note: apps.readiness (051) also decides routing; this migration runs
-- before it exists, so add readiness to route_apps_update when migrations are
-- folded into the baseline. Until then a readiness change reaches the proxy with
-- the next routed-column change or its 60 s fallback check.
CREATE TABLE route_state(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  version INTEGER NOT NULL DEFAULT 0
);
INSERT INTO route_state (id, version) VALUES (1, 0);

CREATE TRIGGER route_apps_insert AFTER INSERT ON apps
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_apps_delete AFTER DELETE ON apps
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_apps_update AFTER UPDATE OF name, domain, host_port, status ON apps
WHEN OLD.name IS NOT NEW.name
  OR OLD.domain IS NOT NEW.domain
  OR OLD.host_port IS NOT NEW.host_port
  OR (OLD.status = 'running') IS NOT (NEW.status = 'running')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_previews_insert AFTER INSERT ON preview_deployments
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_previews_delete AFTER DELETE ON preview_deployments
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_previews_update
AFTER UPDATE OF subdomain, host_port, status, expires_at ON preview_deployments
WHEN OLD.subdomain IS NOT NEW.subdomain
  OR OLD.host_port IS NOT NEW.host_port
  OR OLD.status IS NOT NEW.status
  OR OLD.expires_at IS NOT NEW.expires_at
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_settings_insert AFTER INSERT ON settings
WHEN NEW.key IN ('dashboard_domain', 'preview_domain')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_settings_update AFTER UPDATE ON settings
WHEN NEW.key IN ('dashboard_domain', 'preview_domain') AND OLD.value IS NOT NEW.value
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TRIGGER route_settings_delete AFTER DELETE ON settings
WHEN OLD.key IN ('dashboard_domain', 'preview_domain')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
