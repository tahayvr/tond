-- 012_build_method.sql — add build_method column to apps table
--
-- build_method controls how the app is built from source:
--   "auto"       — detect: use Dockerfile if present, else Nixpacks (default)
--   "nixpacks"   — always use Nixpacks
--   "dockerfile" — always use the Dockerfile in the repo root

ALTER TABLE apps ADD COLUMN build_method TEXT NOT NULL DEFAULT 'auto';
