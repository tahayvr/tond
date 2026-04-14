-- Migration 044: add preview_enabled flag to apps
--
-- When TRUE, pull_request webhook events for this app will automatically
-- trigger preview deployments (Phase 4.6). Defaults to FALSE so existing
-- apps are opt-in only.

ALTER TABLE apps ADD COLUMN preview_enabled INTEGER NOT NULL DEFAULT 0;
