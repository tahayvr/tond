-- GitHub App flow hardening.

-- Pending GitHub App flows (manifest registration and installation). A row is
-- created by a logged-in admin, expires after an hour, and is consumed exactly
-- once by the matching callback from the same admin's browser session.
CREATE TABLE github_app_flows(
  state TEXT PRIMARY KEY NOT NULL,
  -- "manifest" | "install"
  kind TEXT NOT NULL,
  -- github_apps.id for "install" flows; NULL for "manifest" flows
  github_app_id TEXT REFERENCES github_apps(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL
);

-- Registrations are only created once GitHub confirms them; drop the old
-- pending stub rows and the column that correlated them.
DELETE FROM github_apps WHERE app_id = 0;
ALTER TABLE github_apps DROP COLUMN manifest_state;
