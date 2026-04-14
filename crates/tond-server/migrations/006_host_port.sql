-- Add host_port to apps so the proxy can read it from the DB
-- instead of inspecting Docker containers on every route refresh.
ALTER TABLE apps ADD COLUMN host_port INTEGER;
