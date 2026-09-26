-- 020_app_templates.sql — remember which one-click template an app came from
--
-- Set by POST /api/v1/templates/{id}/deploy so the app page can link the
-- template's upgrade notes. NULL for apps created any other way.
ALTER TABLE apps ADD COLUMN template_id TEXT;
ALTER TABLE apps ADD COLUMN template_version TEXT;
