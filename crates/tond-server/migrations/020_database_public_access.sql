-- Opt-in public access for managed databases (H6). Off by default: the
-- container port is published on 127.0.0.1 only. When an admin turns it on,
-- the port is published on 0.0.0.0 and is reachable from the internet.
ALTER TABLE databases ADD COLUMN public_access INTEGER NOT NULL DEFAULT 0;
