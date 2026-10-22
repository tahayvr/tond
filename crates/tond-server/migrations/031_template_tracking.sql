-- Apps deployed from a template remember what they need to follow the
-- catalog and their own domain (handlers/templates.rs, template_apps.rs):
--
--   template_compose_sha256  SHA-256 (hex) of the compose file as Tond last
--                            wrote it from the template; a different hash
--                            means the user edited it, and one-click upgrades
--                            are refused
--   template_releases        JSON object {sha256: template version} of every
--                            template compose file the app has run, so a
--                            rollback to an older one restores its version
--   template_domain_env      JSON {"host": "<host the values were made for>",
--                            "keys": {"<ENV KEY>": "<text with {app_url} or
--                            {app_host}>"}}: env values derived from the app's
--                            address, offered for update when the primary
--                            domain changes
ALTER TABLE apps ADD COLUMN template_compose_sha256 TEXT;
ALTER TABLE apps ADD COLUMN template_releases TEXT;
ALTER TABLE apps ADD COLUMN template_domain_env TEXT;
