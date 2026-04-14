-- 010_drain_timeout.sql — per-app drain timeout for graceful deploys
-- drain_timeout: seconds to wait for the new container to become healthy
--   before cutting traffic over and stopping the old one (default 30 s).
ALTER TABLE apps ADD COLUMN drain_timeout INTEGER NOT NULL DEFAULT 30;
