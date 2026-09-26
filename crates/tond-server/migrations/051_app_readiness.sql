-- How a deploy decides the new container is ready to take over:
--   'http'  health check path, or any HTTP answer on the container port
--           (web apps; the default)
--   'tcp'   the container port accepts TCP connections (non-HTTP services)
--   'none'  background worker: no port, no domain, never routed; ready once
--           the container has stayed up for ready_after_secs without exiting
ALTER TABLE apps ADD COLUMN readiness TEXT NOT NULL DEFAULT 'http';
ALTER TABLE apps ADD COLUMN ready_after_secs INTEGER NOT NULL DEFAULT 10;
