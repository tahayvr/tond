-- 030_inline_sources.sql
-- Adds inline source content columns for the 'dockerfile' and 'compose' source types.
-- When source_type = 'dockerfile', the raw Dockerfile is stored in dockerfile_content.
-- When source_type = 'compose',    the raw docker-compose.yml is stored in compose_content.
ALTER TABLE apps ADD COLUMN dockerfile_content TEXT;
ALTER TABLE apps ADD COLUMN compose_content TEXT;
