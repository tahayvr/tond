-- 013_git_integration.sql
--
-- Adds Phase 4 Git Integration & Automation fields to the apps table:
--
--   webhook_secret  TEXT    — HMAC secret for validating GitHub/GitLab webhook
--                             signatures. NULL means no secret (generic webhooks
--                             or not yet configured).
--   auto_deploy     INTEGER — 1 = automatically trigger a deploy when a webhook
--                             fires for the configured branch; 0 = disabled.
--   watch_branch    TEXT    — Branch to watch for webhook-triggered deploys
--                             (e.g. "main"). NULL means any branch triggers.
--   deploy_key_pub  TEXT    — Public half of the per-app SSH deploy key
--                             (Ed25519, OpenSSH format). NULL = not generated yet.

ALTER TABLE apps ADD COLUMN webhook_secret  TEXT;
ALTER TABLE apps ADD COLUMN auto_deploy     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE apps ADD COLUMN watch_branch    TEXT;
ALTER TABLE apps ADD COLUMN deploy_key_pub  TEXT;
