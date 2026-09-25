-- Last accepted TOTP time step (unix_time / 30) per user. A code is only
-- accepted if its step is strictly greater than this value, so a 6-digit
-- code cannot be replayed within its ±1-step validity window (RFC 6238 §5.2).
-- NULL means no code has been accepted for the current TOTP secret.
ALTER TABLE users ADD COLUMN totp_last_step INTEGER;
