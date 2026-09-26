-- ACME retry state, written by tond-proxy after every issuance attempt so the
-- dashboard can explain why a certificate is still pending and when the next
-- attempt happens. tond-server resets it (failure_count = 0, next_retry_at =
-- NULL) when the domain is saved again or "Retry now" is pressed.
ALTER TABLE certificates ADD COLUMN last_error TEXT;
ALTER TABLE certificates ADD COLUMN last_attempt_at TEXT;
-- RFC 3339; NULL means "try on the next loop iteration".
ALTER TABLE certificates ADD COLUMN next_retry_at TEXT;
-- Consecutive failed attempts; drives the exponential backoff.
ALTER TABLE certificates ADD COLUMN failure_count INTEGER NOT NULL DEFAULT 0;
