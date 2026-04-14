-- Phase 7.1: Password Management
-- Extends sessions with tracking metadata and users with 2FA support.

-- Add user_agent, ip_address, and last_used_at columns to sessions
-- so we can display "Active Sessions" in the dashboard.
ALTER TABLE sessions ADD COLUMN user_agent TEXT;
ALTER TABLE sessions ADD COLUMN ip_address TEXT;
ALTER TABLE sessions ADD COLUMN last_used_at TEXT;

-- Add TOTP 2FA columns to the users table.
-- totp_secret_enc: AES-GCM encrypted TOTP secret (only set after 2FA is enabled)
-- totp_enabled: whether 2FA is currently active for this user
ALTER TABLE users ADD COLUMN totp_secret_enc TEXT;
ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0;

-- Backup codes table: 10 × 16-char hex codes, stored hashed.
-- Each code can only be used once (deleted on use).
CREATE TABLE IF NOT EXISTS totp_backup_codes (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash  TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_totp_backup_codes_user_id
    ON totp_backup_codes(user_id);

-- MFA pending table: short-lived tokens issued after first factor (email/password)
-- when 2FA is enabled. The second factor (TOTP) exchanges this for a full session.
CREATE TABLE IF NOT EXISTS mfa_pending (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
