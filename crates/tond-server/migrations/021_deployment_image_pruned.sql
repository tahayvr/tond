-- Deployments now tag their image per deployment (tond-<app>:<id>). Old
-- images are pruned; this flag records that the image is gone so the
-- dashboard can disable rolling back to it.
ALTER TABLE deployments ADD COLUMN image_pruned INTEGER NOT NULL DEFAULT 0;
