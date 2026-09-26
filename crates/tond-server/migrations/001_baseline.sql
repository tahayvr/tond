-- Tond baseline schema.
--
-- Squashed before the first release (no installs existed): the original
-- migrations 001-049, then Phase 0 (lock down), Phase 1 (solid core) and
-- Phase 2 (observable): request metrics and access logs, the alert engine,
-- host history, uptime checks and heartbeat, certificate issuer, audit
-- shipping; and Phase 3 (v1): compose services, app templates, several
-- domains per app with DNS providers, idempotency keys, managed database
-- essentials, Gitea tokens and the update check. Add new migrations as
-- 002_..., 003_... from here on.

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
  preview_shared_keys TEXT NOT NULL DEFAULT '[]',
  preview_env_vars TEXT NOT NULL DEFAULT '{}',
  readiness TEXT NOT NULL DEFAULT 'http',
  ready_after_secs INTEGER NOT NULL DEFAULT 10,
  access_log INTEGER NOT NULL DEFAULT 0,
  access_log_query INTEGER NOT NULL DEFAULT 0,
  -- Git-sourced compose apps: path of the compose file inside the repository.
  compose_path TEXT,
  -- The one-click template an app came from, set by
  -- POST /api/v1/templates/{id}/deploy so the app page can link the
  -- template's upgrade notes. NULL for apps created any other way.
  template_id TEXT,
  template_version TEXT,
  gitea_token_id TEXT REFERENCES gitea_tokens(id) ON DELETE SET NULL
);

CREATE TABLE deploy_logs(
  id TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL,
  message TEXT NOT NULL,
  level TEXT NOT NULL DEFAULT 'info',
  created_at TEXT NOT NULL,
  trigger_source TEXT NOT NULL DEFAULT 'manual',
  deployment_id TEXT,
  FOREIGN KEY(app_id) REFERENCES apps(id) ON DELETE CASCADE
);
CREATE INDEX idx_apps_name ON apps(name);
CREATE INDEX idx_apps_domain ON apps(domain);

-- Every domain an app answers on. `apps.domain` stays as a read-only mirror
-- of the primary domain (maintained by the triggers below); write here.
CREATE TABLE app_domains(
  domain TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  -- NULL: the app's main container; else a compose service name
  service TEXT,
  -- container port of `service` (NULL: the app's container_port)
  port INTEGER,
  -- "primary" | "alias" | "redirect"
  kind TEXT NOT NULL DEFAULT 'alias' CHECK(kind IN ('primary', 'alias', 'redirect')),
  -- redirect target (another domain of the same app), set iff kind = 'redirect'
  redirect_to TEXT,
  redirect_code INTEGER NOT NULL DEFAULT 308 CHECK(redirect_code IN (301, 308)),
  created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  CHECK((kind = 'redirect') = (redirect_to IS NOT NULL))
);
CREATE INDEX app_domains_app_idx ON app_domains(app_id);
CREATE UNIQUE INDEX app_domains_one_primary ON app_domains(app_id) WHERE kind = 'primary';

-- apps.domain mirrors the primary domain.
CREATE TRIGGER app_domains_primary_insert AFTER INSERT ON app_domains
WHEN NEW.kind = 'primary'
BEGIN
  UPDATE apps SET domain = NEW.domain WHERE id = NEW.app_id;
END;
CREATE TRIGGER app_domains_primary_update AFTER UPDATE ON app_domains
WHEN OLD.kind = 'primary' OR NEW.kind = 'primary'
BEGIN
  UPDATE apps SET domain = (
    SELECT domain FROM app_domains WHERE app_id = NEW.app_id AND kind = 'primary'
  ) WHERE id = NEW.app_id;
END;
CREATE TRIGGER app_domains_primary_delete AFTER DELETE ON app_domains
WHEN OLD.kind = 'primary'
BEGIN
  UPDATE apps SET domain = (
    SELECT domain FROM app_domains WHERE app_id = OLD.app_id AND kind = 'primary'
  ) WHERE id = OLD.app_id;
END;
-- An app created with a domain gets it as its primary domain.
CREATE TRIGGER apps_domain_seed AFTER INSERT ON apps
WHEN NEW.domain IS NOT NULL AND NEW.domain != ''
BEGIN
  INSERT OR IGNORE INTO app_domains(domain, app_id, kind) VALUES (NEW.domain, NEW.id, 'primary');
END;

CREATE TABLE users(
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'admin', -- 'admin' | 'member'
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  totp_secret_enc TEXT,
  totp_enabled INTEGER NOT NULL DEFAULT 0,
  totp_last_step INTEGER,
  disabled_at TEXT,
  last_login_at TEXT
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
  -- "acme" | "custom" (uploaded; never renewed) | "self_signed"
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
  failure_count INTEGER NOT NULL DEFAULT 0,
  issuer TEXT,
  last_renewed_at TEXT,
  -- How the CA validated an ACME certificate: "http-01" | "dns-01".
  challenge TEXT
);

-- DNS hosting APIs Tond may publish ACME DNS-01 TXT records with. The token
-- is write-only: encrypted (secret_key.rs, Purpose::DnsProviderToken) and
-- never returned. tond-proxy reads `zones` (read-only) to decide which
-- domains use DNS-01; it never reads or needs the token.
CREATE TABLE dns_providers(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  -- "cloudflare" | "hetzner" | "digitalocean" | "route53" | "challtestsrv" (test only)
  kind TEXT NOT NULL,
  -- JSON array of zone names this provider may write, e.g. ["example.com"]
  zones TEXT NOT NULL DEFAULT '[]',
  -- non-secret settings (Route 53 access key id, challtestsrv URL), JSON
  config TEXT NOT NULL DEFAULT '{}',
  token_enc TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_used_at TEXT,
  last_error TEXT
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
  image_pruned INTEGER NOT NULL DEFAULT 0,
  kind TEXT NOT NULL DEFAULT 'deploy',
  error TEXT,
  superseded_by TEXT,
  commit_sha TEXT,
  commit_message TEXT,
  commit_author TEXT,
  trigger_detail TEXT,
  rollback_of TEXT,
  phases TEXT NOT NULL DEFAULT '[]',
  failure TEXT,
  -- The compose file a deployment ran (as written by the user or read from
  -- the repository), for rollback.
  compose_source TEXT
);
CREATE INDEX deployments_app_id_idx ON deployments(app_id, created_at DESC);

-- Docker Compose as a first-class app type.
--
-- compose_services: one row per service of a compose app, synced from the
-- compose file on save and on every deploy. User intent (the container port
-- Tond publishes for routing; its domains are app_domains rows with this
-- service set) survives redeploys; what Docker reports (container, state,
-- health) is refreshed on deploy, on service actions and when the dashboard
-- lists the services.
CREATE TABLE compose_services(
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  -- order in the compose file
  position INTEGER NOT NULL DEFAULT 0,
  -- image ref from the compose file (NULL for build-only services)
  image TEXT,
  -- 1 when the service has a build: section
  builds INTEGER NOT NULL DEFAULT 0,
  -- JSON array of container ports the file declares (ports: and expose:)
  ports TEXT NOT NULL DEFAULT '[]',
  -- 1 when the service defines a healthcheck (compose healthchecks are
  -- waited for on deploy)
  has_healthcheck INTEGER NOT NULL DEFAULT 0,
  -- container port Tond publishes on 127.0.0.1:host_port for routing
  published_port INTEGER,
  -- loopback host port allocated for published_port (stable across deploys)
  host_port INTEGER,
  -- container port host_port maps to in the running containers (differs
  -- from published_port until the next deploy after a change)
  routed_port INTEGER,
  container_id TEXT,
  -- "pending" | "running" | "restarting" | "paused" | "created" | "exited"
  -- | "dead" | "missing"
  status TEXT NOT NULL DEFAULT 'pending',
  -- "healthy" | "unhealthy" | "starting"; NULL without a healthcheck
  health TEXT,
  exit_code INTEGER,
  -- the pinned local image the running container uses
  running_image TEXT,
  -- its image id (sha256:...)
  image_digest TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY(app_id, name)
) WITHOUT ROWID;

-- What a compose deployment ran, per service: the image it was built or
-- pulled as and the pinned local tag (tond-<app>/<service>:<image id prefix>) with
-- its image id, so a rollback restores exactly that set.
CREATE TABLE deployment_services(
  deployment_id TEXT NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
  service TEXT NOT NULL,
  -- "build" | "pull" | "reuse" (rollback)
  source TEXT NOT NULL,
  -- image ref from the compose file, or the build context
  source_image TEXT,
  -- pinned local tag
  image TEXT,
  -- image id (sha256:...) of the pinned tag, plus the registry digest of a
  -- pulled image when known ("sha256:...@repo@sha256:...")
  digest TEXT,
  -- "pending" | "building" | "pulling" | "ready" | "starting" | "healthy"
  -- | "running" | "failed"
  status TEXT NOT NULL DEFAULT 'pending',
  PRIMARY KEY(deployment_id, service)
) WITHOUT ROWID;

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
  public_access INTEGER NOT NULL DEFAULT 0,
  -- Per-database backup schedule (tond::cron syntax; NULL = no schedule of
  -- its own) and how many scheduled backups of it to keep.
  backup_schedule TEXT,
  backup_retention INTEGER NOT NULL DEFAULT 7,
  -- Engine major version (e.g. "16") and image variant ("" | "pgvector" |
  -- "postgis"); together they pick the image (tond_common::image_for).
  -- NULL version = the engine's default.
  version TEXT,
  variant TEXT NOT NULL DEFAULT '',
  -- Public access: JSON array of allowed source addresses (IP or CIDR).
  -- Empty = any address. Enforced in the kernel (db_firewall.rs).
  allowlist TEXT NOT NULL DEFAULT '[]',
  -- Serve TLS (postgres, mysql, mariadb) with a certificate mounted from
  -- <data>/databases/.<name>.tls/.
  tls INTEGER NOT NULL DEFAULT 0,
  -- A major-version upgrade in progress or waiting for confirmation
  -- (JSON, db_upgrade::UpgradeState). NULL = none.
  upgrade TEXT
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

-- Gitea/Forgejo access tokens, for commit statuses, PR preview comments and
-- cloning private repositories (like gitlab_tokens).
CREATE TABLE gitea_tokens(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  -- secret_key Purpose::GiteaToken
  token_enc TEXT NOT NULL,
  -- Instance URL, e.g. https://codeberg.org
  base_url TEXT NOT NULL,
  -- Account the token belongs to (from /api/v1/user at creation)
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

CREATE TABLE integrations(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- "telegram" | "discord" | "slack" | "email"
  kind TEXT NOT NULL,
  -- Provider-specific JSON config(see notifications.rs for shapes)
  config TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now')),
  quiet_hours TEXT,
  last_sent_at INTEGER,
  last_error TEXT,
  last_error_at INTEGER
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
  notes TEXT,
  remote_status TEXT,
  remote_error TEXT,
  archive_bytes INTEGER
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

-- Idempotency keys (idempotency.rs): the first successful response to a
-- mutation sent with `Idempotency-Key`, replayed for repeats within 24 h.
-- Scoped per caller (actor), method and concrete path. `status` is NULL
-- while the first request is still running. `response` is the body,
-- encrypted with TOND_SECRET_KEY (it can hold a new token or database
-- password). Bounded: at most a few thousand rows, pruned by age.
CREATE TABLE idempotency_keys (
    actor TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status INTEGER,
    content_type TEXT,
    response TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (actor, method, path, key)
) WITHOUT ROWID;
CREATE INDEX idx_idempotency_keys_created ON idempotency_keys (created_at);

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

CREATE TABLE api_tokens(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  -- visible start of the token("tond_pat_" + 8 chars), safe to show
  prefix TEXT NOT NULL,
  -- sha256 of the full token; the token itself is never stored
  token_hash TEXT NOT NULL UNIQUE,
  -- creator; NULL = created as root on the host(tond token create)
  owner_user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN('personal', 'service', 'agent', 'ci')),
  -- JSON array of scope names
  scopes TEXT NOT NULL,
  -- JSON arrays of app / project ids; both NULL = every resource
  app_ids TEXT,
  project_ids TEXT,
  -- JSON array of IPs / CIDRs; NULL = any address
  allowed_ips TEXT,
  expires_at TEXT,
  last_used_at TEXT,
  last_used_ip TEXT,
  revoked_at TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_api_tokens_owner ON api_tokens(owner_user_id);

CREATE TABLE password_links(
  token_hash TEXT PRIMARY KEY NOT NULL,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  -- "invite" | "reset"
  purpose TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_password_links_user ON password_links(user_id);

CREATE TABLE audit_log(
  -- position in the chain: 1, 2, 3, … assigned by the writer, no gaps
  seq INTEGER PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL,
  -- "user" | "token" | "local_socket" | "system" | "anonymous"
  actor_kind TEXT NOT NULL,
  actor_id TEXT,
  actor_label TEXT NOT NULL,
  action TEXT NOT NULL,
  resource_type TEXT,
  resource_id TEXT,
  resource_name TEXT,
  -- "success" | "failure"
  outcome TEXT NOT NULL DEFAULT 'success',
  ip TEXT,
  detail TEXT,
  prev_hash TEXT NOT NULL,
  -- sha256(prev_hash || canonical row), see audit_log.rs
  hash TEXT NOT NULL
);
CREATE INDEX idx_audit_log_created_at ON audit_log(created_at);
CREATE INDEX idx_audit_log_resource ON audit_log(
  resource_type,
  resource_id,
  seq
);
CREATE INDEX idx_audit_log_actor ON audit_log(actor_label, seq);
CREATE INDEX idx_audit_log_action ON audit_log(action, seq);

CREATE TABLE audit_state(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  checkpoint_seq INTEGER NOT NULL DEFAULT 0,
  checkpoint_hash TEXT NOT NULL,
  pruning INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER audit_log_append_only_update BEFORE UPDATE ON audit_log
BEGIN
  SELECT RAISE(ABORT, 'audit_log is append-only');
END;
CREATE TRIGGER audit_log_append_only_delete BEFORE DELETE ON audit_log
WHEN (SELECT pruning FROM audit_state WHERE id = 1) = 0
BEGIN
  SELECT RAISE(ABORT, 'audit_log rows are only removed by retention');
END;
CREATE INDEX deployments_unfinished_idx ON deployments(
  status,
  created_at
)
WHERE status IN('queued', 'running');

CREATE TABLE route_state(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  version INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER route_apps_insert AFTER INSERT ON apps
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_apps_delete AFTER DELETE ON apps
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_apps_update AFTER UPDATE OF name, domain, host_port, status, readiness ON apps
WHEN OLD.name IS NOT NEW.name
  OR OLD.domain IS NOT NEW.domain
  OR OLD.host_port IS NOT NEW.host_port
  OR OLD.readiness IS NOT NEW.readiness
  OR (OLD.status = 'running') IS NOT (NEW.status = 'running')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_previews_insert AFTER INSERT ON preview_deployments
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_previews_delete AFTER DELETE ON preview_deployments
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_previews_update
AFTER UPDATE OF subdomain, host_port, status, expires_at ON preview_deployments
WHEN OLD.subdomain IS NOT NEW.subdomain
  OR OLD.host_port IS NOT NEW.host_port
  OR OLD.status IS NOT NEW.status
  OR OLD.expires_at IS NOT NEW.expires_at
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_settings_insert AFTER INSERT ON settings
WHEN NEW.key IN ('dashboard_domain', 'preview_domain')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_settings_update AFTER UPDATE ON settings
WHEN NEW.key IN ('dashboard_domain', 'preview_domain') AND OLD.value IS NOT NEW.value
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_settings_delete AFTER DELETE ON settings
WHEN OLD.key IN ('dashboard_domain', 'preview_domain')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
-- The apps domain decides which wildcard certificate tond-proxy orders.
CREATE TRIGGER route_settings_apps_domain_insert AFTER INSERT ON settings
WHEN NEW.key = 'apps_domain'
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_settings_apps_domain_update AFTER UPDATE ON settings
WHEN NEW.key = 'apps_domain' AND OLD.value IS NOT NEW.value
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
-- tond-proxy reloads its routing table when any domain changes.
CREATE TRIGGER route_app_domains_insert AFTER INSERT ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_app_domains_delete AFTER DELETE ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_app_domains_update AFTER UPDATE ON app_domains
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_compose_services_delete AFTER DELETE ON compose_services
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_compose_services_update AFTER UPDATE OF host_port ON compose_services
WHEN OLD.host_port IS NOT NEW.host_port
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_compose_services_insert AFTER INSERT ON compose_services
WHEN NEW.host_port IS NOT NULL
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
-- DNS-01 zones decide how tond-proxy validates certificates.
CREATE TRIGGER route_dns_providers_insert AFTER INSERT ON dns_providers
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_dns_providers_delete AFTER DELETE ON dns_providers
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
CREATE TRIGGER route_dns_providers_zones AFTER UPDATE OF zones ON dns_providers
WHEN OLD.zones IS NOT NEW.zones
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TABLE metric_targets(
  id INTEGER PRIMARY KEY,
  -- "app" | "database" | "cron" | "preview"
  kind TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  -- Latest summed memory limit of the target's containers in bytes(0 = none)
  mem_limit INTEGER NOT NULL DEFAULT 0,
  UNIQUE(kind, owner_id)
);

CREATE TABLE metrics_raw(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE metrics_5m(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  mem_max INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE metrics_1h(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  mem INTEGER NOT NULL,
  mem_max INTEGER NOT NULL,
  rx INTEGER NOT NULL,
  tx INTEGER NOT NULL,
  pids INTEGER NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE metric_events(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  -- "restart" | "oom" | "crash"
  kind TEXT NOT NULL,
  detail TEXT,
  PRIMARY KEY(target, ts, kind)
) WITHOUT ROWID;
CREATE INDEX idx_deploy_logs_deployment ON deploy_logs(
  deployment_id,
  created_at
);

CREATE TABLE database_links(
  database_id TEXT NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  PRIMARY KEY(database_id, app_id)
);
CREATE INDEX idx_database_links_app ON database_links(app_id);

CREATE TABLE requests_1m(
  target INTEGER NOT NULL,
  -- Minute start, unix seconds.
  ts INTEGER NOT NULL,
  requests INTEGER NOT NULL,
  s1xx INTEGER NOT NULL,
  s2xx INTEGER NOT NULL,
  s3xx INTEGER NOT NULL,
  s4xx INTEGER NOT NULL,
  s5xx INTEGER NOT NULL,
  s502 INTEGER NOT NULL,
  s504 INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  -- Latency histograms(tond_common::traffic::encode_histogram): total time
  -- and time to the upstream's response head.
  latency BLOB NOT NULL,
  upstream BLOB NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;

CREATE TABLE requests_1h(
  target INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  requests INTEGER NOT NULL,
  s1xx INTEGER NOT NULL,
  s2xx INTEGER NOT NULL,
  s3xx INTEGER NOT NULL,
  s4xx INTEGER NOT NULL,
  s5xx INTEGER NOT NULL,
  s502 INTEGER NOT NULL,
  s504 INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  latency BLOB NOT NULL,
  upstream BLOB NOT NULL,
  PRIMARY KEY(target, ts)
) WITHOUT ROWID;
CREATE TRIGGER route_apps_access_log AFTER UPDATE OF access_log, access_log_query ON apps
WHEN OLD.access_log IS NOT NEW.access_log
  OR OLD.access_log_query IS NOT NEW.access_log_query
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;

CREATE TABLE alert_rules(
  id TEXT NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  -- tond_common::ALERT_KINDS: cpu | memory | app_down | disk_usage |
  -- disk_forecast | http_5xx_rate | http_p95_latency | cert_expiry | oom |
  -- crash_loop | backup_failed | deploy_failed | cron_failed
  kind TEXT NOT NULL,
  -- App-scoped kinds: one app, or NULL for every app. Ignored otherwise.
  app_id TEXT REFERENCES apps(id) ON DELETE CASCADE,
  -- Unit depends on the kind(percent, ms, days, hours, restarts)
  threshold REAL NOT NULL DEFAULT 0,
  -- Look-back window(5xx rate, p95 latency, OOM, crash loop), seconds
  window_secs INTEGER NOT NULL DEFAULT 0,
  -- How long the condition must hold before the alert fires, seconds
  for_secs INTEGER NOT NULL DEFAULT 0,
  -- info | warning | critical
  severity TEXT NOT NULL DEFAULT 'warning',
  -- Re-notify while firing and unacknowledged; 0 = never
  repeat_secs INTEGER NOT NULL DEFAULT 14400,
  enabled INTEGER NOT NULL DEFAULT 1,
  -- The whole rule is muted until this unix time(NULL = not muted)
  snoozed_until INTEGER,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE alert_rule_notifications(
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, integration_id)
) WITHOUT ROWID;

CREATE TABLE alert_history(
  id TEXT NOT NULL PRIMARY KEY,
  rule_id TEXT REFERENCES alert_rules(id) ON DELETE SET NULL,
  rule_name TEXT NOT NULL,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  -- "app:<id>" | "host" | "cert:<domain>" | "backup" | "cron:<id>"
  target TEXT NOT NULL,
  target_label TEXT NOT NULL,
  app_id TEXT,
  threshold REAL NOT NULL,
  -- Value when it fired, and the latest value seen while firing
  value REAL,
  last_value REAL,
  -- Unix seconds: condition first true, fired, resolved
  pending_since INTEGER NOT NULL,
  fired_at INTEGER NOT NULL,
  resolved_at INTEGER,
  acknowledged_at INTEGER,
  acknowledged_by TEXT,
  last_notified_at INTEGER
);
CREATE INDEX alert_history_fired ON alert_history(fired_at DESC);
CREATE INDEX alert_history_rule ON alert_history(rule_id, fired_at DESC);
CREATE INDEX alert_history_app ON alert_history(app_id, fired_at DESC);
CREATE INDEX alert_history_resolved ON alert_history(resolved_at);

CREATE TABLE alert_states(
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  target TEXT NOT NULL,
  target_label TEXT NOT NULL,
  -- pending | firing | inactive
  state TEXT NOT NULL,
  -- Unix seconds the condition became true
  since INTEGER NOT NULL,
  value REAL,
  episode_id TEXT REFERENCES alert_history(id) ON DELETE SET NULL,
  -- Timed snooze of this target(unix seconds)
  snoozed_until INTEGER,
  PRIMARY KEY(rule_id, target)
) WITHOUT ROWID;

CREATE TABLE alert_deliveries(
  id TEXT NOT NULL PRIMARY KEY,
  episode_id TEXT NOT NULL REFERENCES alert_history(id) ON DELETE CASCADE,
  integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
  -- firing | repeat | resolved
  event TEXT NOT NULL,
  -- pending | sent | failed | skipped
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  -- 1 when held back by quiet hours
  deferred INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  sent_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX alert_deliveries_due ON alert_deliveries(status, next_attempt_at);
CREATE INDEX alert_deliveries_episode ON alert_deliveries(episode_id);

CREATE TABLE host_1m(
  ts INTEGER PRIMARY KEY NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  load1 INTEGER NOT NULL,
  load5 INTEGER NOT NULL,
  load15 INTEGER NOT NULL,
  mem_used INTEGER NOT NULL,
  mem_total INTEGER NOT NULL,
  swap_used INTEGER NOT NULL,
  swap_total INTEGER NOT NULL,
  disk_used INTEGER NOT NULL,
  disk_total INTEGER NOT NULL,
  inodes_used INTEGER NOT NULL,
  inodes_total INTEGER NOT NULL,
  root_used INTEGER NOT NULL,
  root_total INTEGER NOT NULL,
  root_inodes_used INTEGER NOT NULL,
  root_inodes_total INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE host_1h(
  ts INTEGER PRIMARY KEY NOT NULL,
  cpu INTEGER NOT NULL,
  cpu_max INTEGER NOT NULL,
  load1 INTEGER NOT NULL,
  load5 INTEGER NOT NULL,
  load15 INTEGER NOT NULL,
  mem_used INTEGER NOT NULL,
  mem_total INTEGER NOT NULL,
  swap_used INTEGER NOT NULL,
  swap_total INTEGER NOT NULL,
  disk_used INTEGER NOT NULL,
  disk_total INTEGER NOT NULL,
  inodes_used INTEGER NOT NULL,
  inodes_total INTEGER NOT NULL,
  root_used INTEGER NOT NULL,
  root_total INTEGER NOT NULL,
  root_inodes_used INTEGER NOT NULL,
  root_inodes_total INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE uptime_checks(
  id INTEGER PRIMARY KEY,
  -- "app" | "dashboard"
  kind TEXT NOT NULL,
  -- apps.id for "app"; '' for the dashboard
  owner_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  path TEXT NOT NULL DEFAULT '/',
  -- seconds between probes: a multiple of 60, 60 – 3600
  interval_secs INTEGER NOT NULL DEFAULT 60,
  -- expected HTTP status; NULL = any 2xx or 3xx
  expected_status INTEGER,
  timeout_secs INTEGER NOT NULL DEFAULT 10,
  created_at TEXT NOT NULL DEFAULT(datetime('now')),
  UNIQUE(kind, owner_id)
);

CREATE TABLE uptime_1m(
  check_id INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  result INTEGER NOT NULL,
  status INTEGER,
  latency_ms INTEGER,
  PRIMARY KEY(check_id, ts)
) WITHOUT ROWID;

CREATE TABLE uptime_1h(
  check_id INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  probes INTEGER NOT NULL,
  up INTEGER NOT NULL,
  latency_ms INTEGER,
  PRIMARY KEY(check_id, ts)
) WITHOUT ROWID;

CREATE TABLE uptime_incidents(
  id INTEGER PRIMARY KEY,
  check_id INTEGER NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  cause TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_uptime_incidents_check ON uptime_incidents(
  check_id,
  started_at
);
CREATE INDEX idx_uptime_incidents_started ON uptime_incidents(started_at);
CREATE TRIGGER uptime_checks_app_delete AFTER DELETE ON apps
BEGIN
  DELETE FROM uptime_checks WHERE kind = 'app' AND owner_id = OLD.id;
END;
CREATE TRIGGER uptime_checks_delete AFTER DELETE ON uptime_checks
BEGIN
  DELETE FROM uptime_1m WHERE check_id = OLD.id;
  DELETE FROM uptime_1h WHERE check_id = OLD.id;
  DELETE FROM uptime_incidents WHERE check_id = OLD.id;
END;

CREATE TABLE heartbeat_config(
  id TEXT PRIMARY KEY NOT NULL CHECK(id = 'heartbeat'),
  url_enc TEXT NOT NULL DEFAULT '',
  -- "GET" | "POST"
  method TEXT NOT NULL DEFAULT 'GET',
  interval_secs INTEGER NOT NULL DEFAULT 300,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_ok_at INTEGER,
  last_failure_at INTEGER,
  last_error TEXT,
  -- why the last ping was not sent(Tond itself unhealthy), when it was not
  last_skipped_at INTEGER,
  last_skip_reason TEXT
);

CREATE TABLE audit_shipping(
  id TEXT PRIMARY KEY NOT NULL CHECK(id = 'default'),
  syslog_enabled INTEGER NOT NULL DEFAULT 0,
  syslog_host TEXT NOT NULL DEFAULT '',
  syslog_port INTEGER NOT NULL DEFAULT 6514,
  -- 1 = RFC 5425(syslog over TLS), 0 = plain TCP(RFC 6587 framing)
  syslog_tls INTEGER NOT NULL DEFAULT 1,
  webhook_enabled INTEGER NOT NULL DEFAULT 0,
  webhook_url TEXT NOT NULL DEFAULT '',
  -- HMAC-SHA256 key for X-Tond-Signature, encrypted(secret_key.rs)
  webhook_secret TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);

CREATE TABLE audit_ship_state(
  -- "syslog" | "webhook"
  destination TEXT PRIMARY KEY NOT NULL,
  shipped_seq INTEGER NOT NULL DEFAULT 0,
  -- consecutive failed attempts(drives the backoff)
  failures INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_attempt_at TEXT,
  last_success_at TEXT,
  last_error TEXT,
  -- entries removed by retention before they could be delivered
  dropped INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

-- Tond self-update: the cached release check (updates/releases.rs).
--
-- One row. The update run itself is NOT stored here: its status lives in
-- <data>/update/status.json, written by update.sh, because a failed update
-- may restore an older copy of this database.
CREATE TABLE update_check(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  -- Look for new releases at most once a day (folded into the janitor pass).
  enabled INTEGER NOT NULL DEFAULT 1,
  -- Unix seconds of the last attempt, successful or not.
  checked_at INTEGER,
  -- Why the last attempt failed (NULL when it worked).
  error TEXT,
  -- The published releases, newest first: [{version, tag, name, body, url, published_at}]
  releases_json TEXT NOT NULL DEFAULT '[]',
  -- The last finished update run written to the audit log (status.json id).
  audited_run_id TEXT
);

-- Seed rows
INSERT OR IGNORE INTO settings (key, value) VALUES ('dashboard_domain', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('acme_email', '');
INSERT OR IGNORE INTO backup_config (id) VALUES (1);
INSERT INTO audit_state (id, checkpoint_seq, checkpoint_hash)
VALUES (1, 0, '0000000000000000000000000000000000000000000000000000000000000000');
INSERT INTO route_state (id, version) VALUES (1, 0);
INSERT INTO uptime_checks (kind, owner_id, path) VALUES ('dashboard', '', '/health');
INSERT INTO heartbeat_config (id) VALUES ('heartbeat');
INSERT INTO audit_shipping (id) VALUES ('default');
INSERT INTO update_check (id) VALUES (1);
