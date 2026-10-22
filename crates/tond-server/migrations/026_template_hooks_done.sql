-- The template post_deploy steps an app has already run, as a JSON array of
-- fingerprints (deploy/template_setup.rs: SHA-256 of the step, hashed like
-- template_apps::compose_sha256). A template upgrade runs only the steps its
-- new version adds, so an admin account that exists is never seeded twice.
-- admin_setup gains 'upgrade': run the outstanding steps on the next deploy
-- without withholding the app's routes (only 'pending' does that).
ALTER TABLE apps ADD COLUMN template_hooks_done TEXT NOT NULL DEFAULT '[]';
