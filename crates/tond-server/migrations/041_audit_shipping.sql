-- Audit log shipping: copy every audit entry (with its chain hash) to a
-- remote syslog server and/or an HTTPS webhook, so history rewritten on the
-- box itself no longer goes unnoticed. Off by default. See audit_ship.rs.

-- The destinations (one row).
CREATE TABLE audit_shipping(
  id TEXT PRIMARY KEY NOT NULL CHECK(id = 'default'),
  syslog_enabled INTEGER NOT NULL DEFAULT 0,
  syslog_host TEXT NOT NULL DEFAULT '',
  syslog_port INTEGER NOT NULL DEFAULT 6514,
  -- 1 = RFC 5425 (syslog over TLS), 0 = plain TCP (RFC 6587 framing)
  syslog_tls INTEGER NOT NULL DEFAULT 1,
  webhook_enabled INTEGER NOT NULL DEFAULT 0,
  webhook_url TEXT NOT NULL DEFAULT '',
  -- HMAC-SHA256 key for X-Tond-Signature, encrypted (secret_key.rs)
  webhook_secret TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL DEFAULT(datetime('now'))
);
INSERT INTO audit_shipping (id) VALUES ('default');

-- Delivery progress per destination. The queue is audit_log itself: every
-- row with seq > shipped_seq is still to be delivered, so nothing is lost
-- across restarts and the backlog is bounded by audit retention.
CREATE TABLE audit_ship_state(
  -- "syslog" | "webhook"
  destination TEXT PRIMARY KEY NOT NULL,
  shipped_seq INTEGER NOT NULL DEFAULT 0,
  -- consecutive failed attempts (drives the backoff)
  failures INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_attempt_at TEXT,
  last_success_at TEXT,
  last_error TEXT,
  -- entries removed by retention before they could be delivered
  dropped INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
