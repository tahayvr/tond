-- 033_databases_custom_image.sql — Custom Docker image for managed databases
--
-- Adds two nullable columns:
--
--   image_used    TEXT  — the Docker image actually used when the container was
--                         last created/redeployed (e.g. "postgres:16-alpine").
--                         Set on create and on every redeploy. Displayed in the
--                         dashboard so the user knows exactly what is running.
--
--   custom_image  TEXT  — a user-supplied image override. When non-NULL this
--                         value is used instead of the engine default on the
--                         next redeploy (e.g. "postgres:15-alpine").

ALTER TABLE databases ADD COLUMN image_used   TEXT;
ALTER TABLE databases ADD COLUMN custom_image TEXT;
