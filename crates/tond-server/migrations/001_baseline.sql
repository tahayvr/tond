-- Tond baseline schema.
--
-- Squashed before the first release (no installs existed): migrations
-- 001-049 from the original history, then the Phase 0 lock-down migrations
-- (auth hardening, database public access, per-deployment images, webhook
-- hardening, GitHub App flows, certificate retry state, retention indexes).
-- Add new migrations as 002_..., 003_... from here on.

CREATE TABLE apps(
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
  updated_at TEXT NOT NULL,
  host_port INTEGER,
  memory_limit INTEGER,
  cpu_limit INTEGER,
  health_check_path TEXT,
  health_check_interval INTEGER NOT NULL DEFAULT 30,
  health_failure_threshold INTEGER NOT NULL DEFAULT 3,
  health_consecutive_failures INTEGER NOT NULL DEFAULT 0,
  volumes TEXT NOT NULL DEFAULT '[]',
  drain_timeout INTEGER NOT NULL DEFAULT 30,
  build_method TEXT NOT NULL DEFAULT 'auto',
  webhook_secret TEXT,
  auto_deploy INTEGER NOT NULL DEFAULT 0,
  watch_branch TEXT,
  deploy_key_pub TEXT,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  registry_credential_id TEXT REFERENCES registry_credentials(id) ON DELETE SET NULL,
  github_app_id TEXT REFERENCES github_apps(id),
  gitlab_token_id TEXT REFERENCES gitlab_tokens(id) ON DELETE SET NULL,
  dockerfile_content TEXT,
  compose_content TEXT,
  base_dir TEXT,
  static_build INTEGER NOT NULL DEFAULT 0,
  preview_enabled INTEGER NOT NULL DEFAULT 0,
  preview_excluded_keys TEXT NOT NULL DEFAULT '[]',
  preview_env_vars TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE deploy_logs(
  id TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL,
  message TEXT NOT NULL,
  level TEXT NOT NULL DEFAULT 'info',
  created_at TEXT NOT NULL,
  trigger_source TEXT NOT NULL DEFAULT 'manual',
  FOREIGN KEY(app_id) REFERENCES apps(id) ON DELETE CASCADE
);
CREATE INDEX idx_apps_name ON apps(name);
CREATE INDEX idx_apps_domain ON apps(domain);

CREATE TABLE api_keys(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  key_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  last_used_at TEXT
);
CREATE INDEX idx_api_keys_key_hash ON api_keys(key_hash);

CREATE TABLE users(
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'admin', -- 'admin' | 'member'
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  totp_secret_enc TEXT,
  totp_enabled INTEGER NOT NULL DEFAULT 0,
  totp_last_step INTEGER
);
CREATE INDEX idx_users_email ON users(email);

CREATE TABLE sessions(
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  user_agent TEXT,
  ip_address TEXT,
  last_used_at TEXT
);
CREATE INDEX idx_sessions_user_id ON sessions(user_id);

CREATE TABLE certificates(
  id TEXT PRIMARY KEY,
  domain TEXT NOT NULL UNIQUE,
  -- "acme" | "self_signed"
  cert_type TEXT NOT NULL DEFAULT 'acme',
  -- Filesystem paths to PEM files
  cert_path TEXT NOT NULL,
  key_path TEXT NOT NULL,
  -- Expiry tracking for renewal
  issued_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  -- "active" | "pending" | "expired" | "failed"
  status TEXT NOT NULL DEFAULT 'pending',
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  last_error TEXT,
  last_attempt_at TEXT,
  next_retry_at TEXT,
  failure_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE deployments(
  id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL,
  -- Docker image tag produced or pulled by this deployment
  image_tag TEXT,
  -- "pending" | "running" | "succeeded" | "failed"
  status TEXT NOT NULL DEFAULT 'pending',
  started_at TEXT NOT NULL,
  finished_at TEXT,
  created_at TEXT NOT NULL,
  trigger_source TEXT NOT NULL DEFAULT 'manual',
  generated_dockerfile TEXT,
  image_pruned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX deployments_app_id_idx ON deployments(app_id, created_at DESC);

CREATE TABLE preview_deployments(
  id TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  -- Short random hex prefix used to build the subdomain
  -- e.g. "a1b2c3" -> "a1b2c3-myapp.yourdomain.com"
  slug TEXT NOT NULL,
  -- Full subdomain assigned to this preview(slug + "-" + app_name + "." + dashboard_domain)
  subdomain TEXT,
  container_id TEXT,
  host_port INTEGER,
  image_tag TEXT,
  -- TTL in seconds from created_at; background cleaner uses this to purge old rows
  ttl_seconds INTEGER NOT NULL DEFAULT 86400,
  -- "building" | "running" | "failed" | "expired"
  status TEXT NOT NULL DEFAULT 'building',
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  project_preview_id TEXT REFERENCES project_previews(id) ON DELETE SET NULL,
  pr_number INTEGER,
  head_sha TEXT,
  head_ref TEXT,
  pr_url TEXT,
  comment_id INTEGER,
  gitlab_project_id INTEGER
);

CREATE TABLE metrics(
  id TEXT NOT NULL PRIMARY KEY,
  app_id TEXT NOT NULL,
  -- CPU usage as a percentage(0.0 – 100.0)
  cpu_percent REAL NOT NULL DEFAULT 0.0,
  -- Memory used by the container in bytes
  mem_used INTEGER NOT NULL DEFAULT 0,
  -- Memory limit for the container in bytes
  mem_limit INTEGER NOT NULL DEFAULT 0,
  -- Cumulative bytes received on all network interfaces
  net_rx INTEGER NOT NULL DEFAULT 0,
  -- Cumulative bytes transmitted on all network interfaces
  net_tx INTEGER NOT NULL DEFAULT 0,
  -- Number of container restarts recorded at this sample
  restarts INTEGER NOT NULL DEFAULT 0,
  collected_at TEXT NOT NULL DEFAULT(datetime('now'))
);
CREATE INDEX idx_metrics_app_collected
ON metrics(app_id, collected_at DESC);

CREATE TABLE alert_rules(
  id TEXT NOT NULL PRIMARY KEY,
  -- Optional: scope to a specific app; NULL = all apps
  app_id TEXT REFERENCES apps(id) ON DELETE CASCADE,
  -- Human-readable name
  name TEXT NOT NULL,
  -- Metric to watch: "cpu" | "memory" | "app_down"
  metric TEXT NOT NULL,
  -- Threshold:
  -- cpu / memory → percentage(0–100)
  -- app_down → minutes the app must be non-running before alerting
  threshold REAL NOT NULL,
  -- Webhook URL to POST to when the condition fires
  webhook_url TEXT NOT NULL,
  -- 1 = active, 0 = paused
  enabled INTEGER NOT NULL DEFAULT 1,
  -- ISO-8601 timestamp of last fire(NULL = never fired)
  last_fired_at TEXT,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  events TEXT
);

CREATE TABLE audit_log(
  id TEXT PRIMARY KEY,
  app_id TEXT,
  app_name TEXT,
  action TEXT NOT NULL,
  actor TEXT NOT NULL,
  detail TEXT,
  created_at TEXT NOT NULL DEFAULT(datetime('now'))
);
CREATE INDEX idx_audit_log_app_id ON audit_log(app_id);
CREATE INDEX idx_audit_log_created_at ON audit_log(created_at DESC);

CREATE TABLE projects(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL UNIQUE,
  description TEXT,
  env_vars TEXT NOT NULL DEFAULT '{}', -- JSON object of shared env vars
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE databases(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL UNIQUE,
  db_type TEXT NOT NULL,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  container_id TEXT,
  host_port INTEGER,
  status TEXT NOT NULL DEFAULT 'pending',
  db_name TEXT,
  db_user TEXT,
  db_password TEXT,
  volume_path TEXT,
  memory_limit INTEGER,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  custom_external_url TEXT,
  custom_internal_url TEXT,
  image_used TEXT,
  custom_image TEXT,
  root_password TEXT,
  public_access INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE cron_jobs(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL UNIQUE,
  schedule TEXT NOT NULL, -- cron expression, e.g. "0 * * * *"
  image TEXT NOT NULL, -- Docker image to run
  command TEXT, -- optional override command(JSON array or plain string)
  project_id TEXT, -- optional project association(FK to projects.id)
  enabled INTEGER NOT NULL DEFAULT 1, -- 1 = enabled, 0 = paused
  last_run_at TEXT, -- ISO-8601 of last execution
  next_run_at TEXT, -- ISO-8601 of next scheduled execution
  status TEXT NOT NULL DEFAULT 'idle', -- "idle" | "running" | "failed"
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  timeout_seconds INTEGER,
  env TEXT,
  last_run_status TEXT,
  concurrency_policy TEXT NOT NULL DEFAULT 'skip',
  memory_mb INTEGER,
  cpu_shares INTEGER,
  timezone TEXT,
  max_retries INTEGER NOT NULL DEFAULT 0,
  retry_delay_seconds INTEGER NOT NULL DEFAULT 60,
  volumes TEXT
);

CREATE TABLE cron_job_runs(
  id TEXT PRIMARY KEY NOT NULL,
  cron_job_id TEXT NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
  status TEXT NOT NULL, -- "running" | "success" | "failed"
  exit_code INTEGER,
  output TEXT, -- last N lines of container stdout+stderr
  started_at TEXT NOT NULL,
  finished_at TEXT,
  attempt INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_apps_status
ON apps(status);
CREATE INDEX idx_cron_jobs_next_run_at
ON cron_jobs(next_run_at);
CREATE INDEX idx_preview_deployments_expires_at
ON preview_deployments(
  expires_at
);

CREATE TABLE registry_credentials(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  server TEXT NOT NULL,
  username TEXT NOT NULL,
  -- AES-256-GCM encrypted: base64(nonce || ciphertext)
  password_enc TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE github_apps(
  id TEXT PRIMARY KEY,
  -- Human-readable name chosen during manifest creation(e.g. "tond-myserver")
  name TEXT NOT NULL,
  -- GitHub numeric App ID(returned by the manifest callback)
  app_id INTEGER NOT NULL,
  -- OAuth client_id(returned by the manifest callback)
  client_id TEXT NOT NULL,
  -- OAuth client_secret(returned by the manifest callback, store encrypted in future)
  client_secret TEXT NOT NULL,
  -- HMAC-SHA256 secret for validating webhook payloads from this App
  webhook_secret TEXT NOT NULL,
  -- RSA PEM private key(returned by the manifest callback) — used to sign JWTs
  private_key TEXT NOT NULL,
  -- GitHub installation ID(written once the user installs the App on their org/repos)
  installation_id INTEGER,
  -- GitHub App slug(e.g. "tond-myserver") — used to build the install URL
  slug TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE gitlab_tokens(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  token_enc TEXT NOT NULL,
  gitlab_url TEXT NOT NULL DEFAULT 'https://gitlab.com',
  username TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE totp_backup_codes(
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  code_hash TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_totp_backup_codes_user_id
ON totp_backup_codes(user_id);

CREATE TABLE mfa_pending(
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE alert_rule_notifications(
  id TEXT NOT NULL PRIMARY KEY,
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  integration_id TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE integrations(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- "telegram" | "discord" | "slack" | "email"
  kind TEXT NOT NULL,
  -- Provider-specific JSON config(see notifications.rs for shapes)
  config TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE storage_providers(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- "s3"(covers AWS S3, Cloudflare R2, Backblaze B2, MinIO, etc.)
  kind TEXT NOT NULL DEFAULT 's3',
  -- Provider-specific JSON config(see storage.rs for shape)
  config TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE snapshots(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- "pending" | "running" | "completed" | "failed"
  status TEXT NOT NULL DEFAULT 'pending',
  -- "scheduled" | "manual" | "pre_deploy" | "pre_restore"
  trigger TEXT NOT NULL DEFAULT 'manual',
  -- "full" | "app:{name}" | "database:{name}"
  scope TEXT NOT NULL DEFAULT 'full',
  -- set when trigger = "pre_deploy"
  trigger_app TEXT,
  deployment_id TEXT REFERENCES deployments(id) ON DELETE SET NULL,
  -- previous snapshot used as hard-link base
  parent_snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  completed_at TEXT,
  -- bytes physically written(not counting hard-linked files)
  delta_bytes INTEGER,
  -- total logical size if all hard links were resolved
  total_bytes INTEGER,
  -- absolute path to local snapshot directory
  local_path TEXT,
  -- object key on the remote storage provider(if uploaded)
  remote_key TEXT,
  storage_provider_id TEXT REFERENCES storage_providers(id) ON DELETE SET NULL,
  -- SHA-256 of the exported .tar.gz(if uploaded)
  checksum_sha256 TEXT,
  error_message TEXT,
  notes TEXT
);
CREATE INDEX snapshots_trigger_app ON snapshots(trigger_app, created_at DESC);
CREATE INDEX snapshots_created_at ON snapshots(created_at DESC);

CREATE TABLE backup_config(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  enabled INTEGER NOT NULL DEFAULT 0,
  full_schedule_cron TEXT NOT NULL DEFAULT '0 3 * * *',
  full_retention_count INTEGER NOT NULL DEFAULT 7,
  pre_deploy_enabled INTEGER NOT NULL DEFAULT 1,
  pre_deploy_retention_per_app INTEGER NOT NULL DEFAULT 10,
  keep_local INTEGER NOT NULL DEFAULT 1,
  storage_provider_id TEXT REFERENCES storage_providers(id) ON DELETE SET NULL
);

CREATE TABLE project_previews(
  id TEXT PRIMARY KEY NOT NULL,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  -- Shared slug used as the prefix for every member app's subdomain
  slug TEXT NOT NULL,
  -- TTL in seconds; mirrors the ttl_seconds on child preview_deployments
  ttl_seconds INTEGER NOT NULL DEFAULT 86400,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE setup_tokens(
  token_hash TEXT PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE terminal_tickets(
  ticket_hash TEXT PRIMARY KEY NOT NULL,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  origin TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE webhook_deliveries(
  key TEXT PRIMARY KEY NOT NULL,
  received_at TEXT NOT NULL
);
CREATE INDEX idx_webhook_deliveries_received_at ON webhook_deliveries(
  received_at
);

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
CREATE INDEX idx_deploy_logs_created_at ON deploy_logs(created_at);
CREATE INDEX idx_deploy_logs_app_created ON deploy_logs(app_id, created_at);
CREATE INDEX idx_cron_job_runs_job_started ON cron_job_runs(
  cron_job_id,
  started_at
);
CREATE INDEX idx_cron_job_runs_running ON cron_job_runs(
  status
) WHERE status = 'running';
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_metrics_collected_at ON metrics(collected_at);


-- Seed rows
INSERT OR IGNORE INTO settings (key, value) VALUES ('dashboard_domain', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('acme_email', '');
INSERT OR IGNORE INTO backup_config (id) VALUES (1);
