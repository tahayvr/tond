-- 015_metrics.sql
-- Per-container resource metrics collected every 30 seconds.
-- Pruned automatically after 7 days.
CREATE TABLE IF NOT EXISTS metrics (
    id          TEXT    NOT NULL PRIMARY KEY,
    app_id      TEXT    NOT NULL,
    -- CPU usage as a percentage (0.0 – 100.0)
    cpu_percent REAL    NOT NULL DEFAULT 0.0,
    -- Memory used by the container in bytes
    mem_used    INTEGER NOT NULL DEFAULT 0,
    -- Memory limit for the container in bytes
    mem_limit   INTEGER NOT NULL DEFAULT 0,
    -- Cumulative bytes received on all network interfaces
    net_rx      INTEGER NOT NULL DEFAULT 0,
    -- Cumulative bytes transmitted on all network interfaces
    net_tx      INTEGER NOT NULL DEFAULT 0,
    -- Number of container restarts recorded at this sample
    restarts    INTEGER NOT NULL DEFAULT 0,
    collected_at TEXT   NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_metrics_app_collected
    ON metrics (app_id, collected_at DESC);
