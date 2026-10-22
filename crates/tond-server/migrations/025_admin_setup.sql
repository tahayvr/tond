-- How a template app's admin account comes to exist (see
-- crates/tond-server/src/template_setup.rs):
--   'pending'    post_deploy commands will create it on the next successful start
--   'done'       they did
--   'failed'     they gave up (admin_setup_error says why): the app page warns
--   'open'       the first visitor creates it: the app page warns
--   'dismissed'  the warning was dismissed
-- NULL for apps that didn't come from such a template.
ALTER TABLE apps ADD COLUMN admin_setup TEXT;
ALTER TABLE apps ADD COLUMN admin_setup_error TEXT;

-- tond-proxy withholds a compose app's routes while its admin_setup is
-- 'pending' (tond_common::route_queries::COMPOSE_SERVICE_ROUTES), so a
-- change into or out of 'pending' is a routing change.
CREATE TRIGGER route_apps_admin_setup AFTER UPDATE OF admin_setup ON apps
WHEN (OLD.admin_setup IS 'pending') IS NOT (NEW.admin_setup IS 'pending')
BEGIN
  UPDATE route_state SET version = version + 1 WHERE id = 1;
END;
