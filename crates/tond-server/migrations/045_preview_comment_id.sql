-- Migration 045: add comment_id to preview_deployments
--
-- Stores the GitHub issue comment ID posted to the PR thread after a preview
-- deploy completes. Used to PATCH (update) the comment instead of creating a
-- new one when the same PR re-deploys.

ALTER TABLE preview_deployments ADD COLUMN comment_id INTEGER;
