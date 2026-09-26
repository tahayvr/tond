-- Explicit database access for apps outside the database's project.
--
-- Every app, database, cron job and preview runs on its own Docker network
-- (project members share the project network). A link attaches the database
-- container to the app's network, so the app reaches the database by its
-- name while apps linked to the same database still cannot reach each other.
CREATE TABLE database_links(
  database_id TEXT NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (database_id, app_id)
);
CREATE INDEX idx_database_links_app ON database_links(app_id);
