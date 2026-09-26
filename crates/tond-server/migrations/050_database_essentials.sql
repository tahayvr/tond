-- Phase 3: managed database essentials.

-- Per-database backup schedule (tond::cron syntax; NULL = no schedule of its
-- own) and how many scheduled backups of it to keep.
ALTER TABLE databases ADD COLUMN backup_schedule TEXT;
ALTER TABLE databases ADD COLUMN backup_retention INTEGER NOT NULL DEFAULT 7;

-- Engine major version (e.g. "16") and image variant ("" | "pgvector" |
-- "postgis"); together they pick the image (tond_common::image_for).
-- NULL version = the engine's default.
ALTER TABLE databases ADD COLUMN version TEXT;
ALTER TABLE databases ADD COLUMN variant TEXT NOT NULL DEFAULT '';

-- Public access: JSON array of allowed source addresses (IP or CIDR). Empty
-- = any address. Enforced in the kernel (db_firewall.rs).
ALTER TABLE databases ADD COLUMN allowlist TEXT NOT NULL DEFAULT '[]';
-- Serve TLS (postgres, mysql, mariadb) with a certificate mounted from
-- <data>/databases/.<name>.tls/.
ALTER TABLE databases ADD COLUMN tls INTEGER NOT NULL DEFAULT 0;

-- A major-version upgrade in progress or waiting for confirmation
-- (JSON, db_upgrade::UpgradeState). NULL = none.
ALTER TABLE databases ADD COLUMN upgrade TEXT;
