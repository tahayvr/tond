-- 007_resource_limits.sql — add resource limit columns to apps
--
-- memory_limit: max memory in bytes (NULL = no limit; default 256 MiB = 268435456)
-- cpu_limit:    max CPU as nano_cpus (NULL = no limit; default 0.5 CPU = 500000000)
--
-- NULL means "no limit applied" for rows created before this migration.
-- App::new() sets the defaults in Rust so new apps get sensible values.

ALTER TABLE apps ADD COLUMN memory_limit INTEGER;
ALTER TABLE apps ADD COLUMN cpu_limit INTEGER;
