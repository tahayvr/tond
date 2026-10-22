-- When each backup schedule last ran, so a backup that came due while the
-- server was down runs once at the next start (a catch-up) instead of being
-- skipped (backup.rs: backup_scheduler).
--   job: "full" | "database:<name>"
--   schedule: the cron expression last_slot belongs to (a changed schedule
--             starts over from now instead of catching up)
--   last_slot: the latest slot that ran or was caught up (RFC 3339, UTC)
CREATE TABLE backup_schedule_state(
  job TEXT PRIMARY KEY NOT NULL,
  schedule TEXT NOT NULL,
  last_slot TEXT NOT NULL
);
