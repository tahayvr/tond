-- Migration 029: Add manifest_state to github_apps (Phase 4.4 refactor)
--
-- Adds a `manifest_state` column that stores the UUID generated at manifest
-- creation time. GitHub echoes it back in the `setup_url` redirect after
-- installation, allowing us to route the installation_id to the exact app row
-- instead of always picking the most-recently-created row.

ALTER TABLE github_apps ADD COLUMN manifest_state TEXT;
