-- Apps table
CREATE TABLE IF NOT EXISTS apps (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    domain TEXT,
    source_type TEXT NOT NULL DEFAULT 'git',
    source_url TEXT,
    branch TEXT,
    container_port INTEGER NOT NULL DEFAULT 3000,
    container_id TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    env_vars TEXT DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Deploy logs
CREATE TABLE IF NOT EXISTS deploy_logs (
    id TEXT PRIMARY KEY NOT NULL,
    app_id TEXT NOT NULL,
    message TEXT NOT NULL,
    level TEXT NOT NULL DEFAULT 'info',
    created_at TEXT NOT NULL,
    FOREIGN KEY (app_id) REFERENCES apps(id) ON DELETE CASCADE
);

-- Index for fast lookups
CREATE INDEX IF NOT EXISTS idx_apps_name ON apps(name);
CREATE INDEX IF NOT EXISTS idx_apps_domain ON apps(domain);
CREATE INDEX IF NOT EXISTS idx_deploy_logs_app_id ON deploy_logs(app_id);
