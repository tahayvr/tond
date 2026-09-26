-- Certificates view: who issued the certificate and when it was last
-- (re)issued successfully. Both come from tond-proxy's cert events.
ALTER TABLE certificates ADD COLUMN issuer TEXT;
ALTER TABLE certificates ADD COLUMN last_renewed_at TEXT;
