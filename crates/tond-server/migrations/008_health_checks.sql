ALTER TABLE apps ADD COLUMN health_check_path TEXT;
ALTER TABLE apps ADD COLUMN health_check_interval INTEGER NOT NULL DEFAULT 30;
ALTER TABLE apps ADD COLUMN health_failure_threshold INTEGER NOT NULL DEFAULT 3;
ALTER TABLE apps ADD COLUMN health_consecutive_failures INTEGER NOT NULL DEFAULT 0;
