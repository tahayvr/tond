-- 032_databases_custom_urls.sql — Custom URL overrides for managed databases
--
-- Adds two nullable columns that let users supply their own connection string
-- values. When non-NULL, these take precedence over the auto-computed strings
-- that build_detail() derives from host_port / credentials.
--
-- custom_external_url:  overrides the computed "localhost:<host_port>/..." string
-- custom_internal_url:  overrides the computed "alias:<container_port>/..." string

ALTER TABLE databases ADD COLUMN custom_external_url TEXT;
ALTER TABLE databases ADD COLUMN custom_internal_url  TEXT;
