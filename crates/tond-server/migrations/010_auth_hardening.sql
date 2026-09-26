-- Auth hardening (Phase 0 lock-down).
--
-- Sessions and pending 2FA logins are now looked up by the SHA-256 hash of the
-- cookie value, so rows created with raw IDs can never match again: drop them.
DELETE FROM sessions;
DELETE FROM mfa_pending;

-- Wrong TOTP codes submitted against one pending login; the pending login is
-- discarded once the cap is reached and the user has to enter the password again.
ALTER TABLE mfa_pending ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;

-- First-run setup token. Only the SHA-256 hash is stored; tond-server writes
-- the token itself to a root-only file and the installer prints the setup link.
-- The row is consumed in the same transaction that creates the first admin.
CREATE TABLE setup_tokens(
  token_hash TEXT PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);

-- Single-use, short-lived tickets that authorize one terminal WebSocket.
-- Bound to the user, the target and the dashboard origin that requested them.
CREATE TABLE terminal_tickets(
  ticket_hash TEXT PRIMARY KEY NOT NULL,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  origin TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL
);
