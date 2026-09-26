-- Phase 1 authorization: RBAC roles, scoped API tokens, user lifecycle and a
-- hash-chained, append-only audit log. (Pre-release: no data to carry over.)

-- ── Roles: admin | deployer | viewer (was admin | member) ──
UPDATE users SET role = 'deployer' WHERE role NOT IN ('admin', 'deployer', 'viewer');
ALTER TABLE users ADD COLUMN disabled_at TEXT;
ALTER TABLE users ADD COLUMN last_login_at TEXT;

-- ── Scoped API tokens replace the all-powerful api_keys ──
DROP TABLE api_keys;

CREATE TABLE api_tokens(
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  -- visible start of the token ("tond_pat_" + 8 chars), safe to show
  prefix TEXT NOT NULL,
  -- sha256 of the full token; the token itself is never stored
  token_hash TEXT NOT NULL UNIQUE,
  -- creator; NULL = created as root on the host (tond token create)
  owner_user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('personal', 'service', 'agent', 'ci')),
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

-- ── One-time set-password links (invites and admin resets), valid 24 h ──
CREATE TABLE password_links(
  token_hash TEXT PRIMARY KEY NOT NULL,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  -- "invite" | "reset"
  purpose TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_password_links_user ON password_links(user_id);

-- ── Audit log: append-only hash chain ──
DROP TABLE audit_log;

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
CREATE INDEX idx_audit_log_resource ON audit_log(resource_type, resource_id, seq);
CREATE INDEX idx_audit_log_actor ON audit_log(actor_label, seq);
CREATE INDEX idx_audit_log_action ON audit_log(action, seq);

-- Chain state: where the retained chain starts after retention pruned older
-- rows, and a flag the janitor raises while it prunes.
CREATE TABLE audit_state(
  id INTEGER PRIMARY KEY CHECK (id = 1),
  checkpoint_seq INTEGER NOT NULL DEFAULT 0,
  checkpoint_hash TEXT NOT NULL,
  pruning INTEGER NOT NULL DEFAULT 0
);
INSERT INTO audit_state (id, checkpoint_seq, checkpoint_hash)
VALUES (1, 0, '0000000000000000000000000000000000000000000000000000000000000000');

CREATE TRIGGER audit_log_append_only_update BEFORE UPDATE ON audit_log
BEGIN
  SELECT RAISE(ABORT, 'audit_log is append-only');
END;

CREATE TRIGGER audit_log_append_only_delete BEFORE DELETE ON audit_log
WHEN (SELECT pruning FROM audit_state WHERE id = 1) = 0
BEGIN
  SELECT RAISE(ABORT, 'audit_log rows are only removed by retention');
END;
