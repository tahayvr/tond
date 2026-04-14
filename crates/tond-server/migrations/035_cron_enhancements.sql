-- Phase 6.4 enhancements: timeout and environment variables for cron jobs

-- Maximum wall-clock time (seconds) the container is allowed to run.
-- NULL means no timeout (wait forever — the previous behavior).
ALTER TABLE cron_jobs ADD COLUMN timeout_seconds INTEGER;

-- Environment variables passed to the container, stored as a JSON object
-- string, e.g. '{"API_KEY":"secret","REGION":"eu-west"}'. NULL means none.
ALTER TABLE cron_jobs ADD COLUMN env TEXT;
