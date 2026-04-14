-- 011_deployments.sql — deployment history table
--
-- Tracks every deployment attempt for an app. A row is inserted when a
-- deploy starts and updated when it finishes (successfully or not).
-- The `image_tag` column records the Docker image used so that rollbacks
-- can re-run the exact same image without rebuilding.

CREATE TABLE IF NOT EXISTS deployments (
    id          TEXT    PRIMARY KEY,
    app_id      TEXT    NOT NULL,
    -- Docker image tag produced or pulled by this deployment
    image_tag   TEXT,
    -- "pending" | "running" | "succeeded" | "failed"
    status      TEXT    NOT NULL DEFAULT 'pending',
    started_at  TEXT    NOT NULL,
    finished_at TEXT,
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS deployments_app_id_idx ON deployments (app_id, created_at DESC);
