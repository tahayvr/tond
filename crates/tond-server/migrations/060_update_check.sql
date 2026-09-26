-- Tond self-update: the cached release check (updates/releases.rs).
--
-- One row. The update run itself is NOT stored here: its status lives in
-- <data>/update/status.json, written by update.sh, because a failed update
-- may restore an older copy of this database.
CREATE TABLE update_check(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  -- Look for new releases at most once a day (folded into the janitor pass).
  enabled INTEGER NOT NULL DEFAULT 1,
  -- Unix seconds of the last attempt, successful or not.
  checked_at INTEGER,
  -- Why the last attempt failed (NULL when it worked).
  error TEXT,
  -- The published releases, newest first: [{version, tag, name, body, url, published_at}]
  releases_json TEXT NOT NULL DEFAULT '[]',
  -- The last finished update run written to the audit log (status.json id).
  audited_run_id TEXT
);

INSERT INTO update_check(id) VALUES(1);
