-- Snapshot remote copies are uploaded in the background as encrypted,
-- streamed archives (see backup.rs / snapshot_archive.rs).
--   remote_status: NULL (local only) | 'uploading' | 'uploaded' | 'failed'
--   remote_error:  why the last upload failed
--   archive_bytes: size of the encrypted archive in remote storage
-- checksum_sha256 now holds the SHA-256 of that encrypted archive.
ALTER TABLE snapshots ADD COLUMN remote_status TEXT;
ALTER TABLE snapshots ADD COLUMN remote_error TEXT;
ALTER TABLE snapshots ADD COLUMN archive_bytes INTEGER;
