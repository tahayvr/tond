// snapshot_files.rs — local snapshot copies, encrypted at rest
//
// A local snapshot is a directory under <data>/backups/snapshots/<id> whose
// files are each encrypted on their own with the streaming format of
// snapshot_archive ("TONDLOC1", key derived from TOND_SECRET_KEY for
// `Purpose::LocalSnapshotFile`). Directory names and file names stay
// readable (they are app, volume and database names); contents don't.
//
// - Deduplication still works: an encrypted file's size is a function of
//   the plaintext size (`snapshot_archive::encrypted_len`) and it keeps the
//   source's modification time, so a file whose size and mtime haven't
//   changed is hard-linked from the previous snapshot instead of encrypted
//   again.
// - Owners, modes and modification times of the sources are kept on the
//   encrypted files and put back on restore.
// - Memory stays flat: files stream through 64 KiB records.
// - Restores decrypt straight into the destination (`decrypt_tree`), or,
//   for dumps that must be handed to a container, into a temporary working
//   directory (`plain_subset`) that is removed when dropped.
// - Key rotation (TOND_PREVIOUS_SECRET_KEY set) re-encrypts every local file
//   that only the previous key opens (`rotate_local_snapshots`), keeping
//   hard links between snapshots. Remote archives and exports are not
//   rotated: restores try both keys, so keep the previous key until they
//   have aged out (see docs/content/concepts/backups.md).
// - Snapshots taken before local copies were encrypted are refused with a
//   clear error (pre-release: there are no such installs to migrate).

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::secret_key::{self, Purpose};
use crate::snapshot_archive::{self as sa, DecryptReader, Kind};

/// Local snapshot files.
pub fn local_kind() -> Kind<'static> {
    Kind {
        magic: sa::LOCAL_MAGIC,
        info: Purpose::LocalSnapshotFile.aad(),
    }
}

/// Snapshot archives (remote copies and exports).
pub fn archive_kind() -> Kind<'static> {
    Kind {
        magic: sa::ARCHIVE_MAGIC,
        info: Purpose::SnapshotArchive.aad(),
    }
}

/// Keys that may decrypt snapshot data: TOND_SECRET_KEY and, during a key
/// rotation, the previous one.
pub fn secrets() -> Vec<String> {
    let mut secrets = vec![secret_key::current_secret()];
    secrets.extend(secret_key::previous_secret());
    secrets
}

fn secret_refs(secrets: &[String]) -> Vec<&str> {
    secrets.iter().map(String::as_str).collect()
}

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string()))
}

/// <data>/backups/.work: temporary plaintext working directories.
fn work_root() -> PathBuf {
    data_dir().join("backups").join(".work")
}

// ── Metadata ─────────────────────────────────────────────────────────────────

/// Owner and mode of `meta` onto `path` (owners: best-effort, only root can).
fn copy_owner_and_mode(meta: &std::fs::Metadata, path: &Path) {
    let _ = std::os::unix::fs::lchown(path, Some(meta.uid()), Some(meta.gid()));
    let _ = std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(meta.mode() & 0o7777 & !0o6000),
    );
}

fn copy_mtime(meta: &std::fs::Metadata, path: &Path) {
    if let Ok(mtime) = meta.modified() {
        let _ = File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(mtime));
    }
}

// ── Encrypting ───────────────────────────────────────────────────────────────

/// Encrypt the plaintext file `src` into `dest`, keeping its owner, mode and
/// modification time. Returns the plaintext size.
pub fn encrypt_file(src: &Path, dest: &Path) -> Result<u64> {
    let meta = std::fs::metadata(src)?;
    let input = File::open(src).with_context(|| format!("failed to open {}", src.display()))?;
    let n = sa::encrypt_to_file(input, dest, &secret_key::current_secret(), local_kind())?;
    copy_owner_and_mode(&meta, dest);
    copy_mtime(&meta, dest);
    Ok(n)
}

/// Write `bytes` as a new encrypted file (manifests, metadata).
pub fn write_encrypted(dest: &Path, bytes: &[u8]) -> Result<()> {
    sa::encrypt_to_file(bytes, dest, &secret_key::current_secret(), local_kind())?;
    Ok(())
}

/// Encrypt the plaintext file at `path` in place (a dump just written by an
/// engine): the encrypted copy replaces it atomically.
pub fn seal_in_place(path: &Path) -> Result<()> {
    let tmp = sibling(path, ".sealing");
    let result = encrypt_file(path, &tmp).and_then(|_| {
        std::fs::rename(&tmp, path)?;
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(path);
    }
    result
}

/// `dir/<prefix><name>` next to `path`.
fn sibling(path: &Path, prefix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{prefix}{name}"))
}

/// Copy the tree `src` into `dest` encrypting every regular file, and
/// hard-link files whose size and modification time haven't changed since
/// the previous snapshot `prev`. Only regular files and directories are
/// copied: symlinks (which a container could point at host files), sockets
/// and devices are skipped. Returns (bytes written, total bytes) in
/// plaintext sizes.
pub fn encrypt_tree(src: &Path, dest: &Path, prev: Option<&Path>) -> Result<(i64, i64)> {
    let mut delta: i64 = 0;
    let mut total: i64 = 0;
    let mut dirs: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let dest_path = dest.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            if let Ok(meta) = entry.metadata() {
                dirs.push((dest_path, meta));
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let src_meta = entry.metadata()?;
        let size = src_meta.len() as i64;
        total += size;
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let hard_linked = prev.is_some_and(|prev_dir| {
            let prev_path = prev_dir.join(rel);
            match std::fs::symlink_metadata(&prev_path) {
                Ok(prev_meta) if prev_meta.is_file() => {
                    src_meta.modified().ok() == prev_meta.modified().ok()
                        && prev_meta.len() == sa::encrypted_len(src_meta.len(), 0)
                        && std::fs::hard_link(&prev_path, &dest_path).is_ok()
                }
                _ => false,
            }
        });
        if !hard_linked {
            encrypt_file(entry.path(), &dest_path)
                .with_context(|| format!("failed to copy {}", rel.display()))?;
            delta += size;
        }
    }
    // Directory owners and modes last, so a read-only directory doesn't stop
    // its own contents from being written.
    for (path, meta) in dirs.iter().rev() {
        copy_owner_and_mode(meta, path);
    }
    Ok((delta, total))
}

// ── Decrypting ───────────────────────────────────────────────────────────────

/// Open a local snapshot file for reading.
pub fn open(path: &Path) -> Result<DecryptReader<File>> {
    let secrets = secrets();
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    DecryptReader::open(file, &secret_refs(&secrets), local_kind(), None)
        .with_context(|| format!("can't read the snapshot file {}", path.display()))
}

/// The whole plaintext of a (small) local snapshot file.
pub fn read_encrypted(path: &Path) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    open(path)?
        .read_to_end(&mut out)
        .with_context(|| format!("can't read the snapshot file {}", path.display()))?;
    Ok(out)
}

/// Decrypt the local snapshot file `src` into `dest`, putting the owner,
/// mode and modification time back. Returns the plaintext size.
pub fn decrypt_file(src: &Path, dest: &Path) -> Result<u64> {
    let meta = std::fs::metadata(src)?;
    let mut reader = open(src)?;
    let mut out =
        File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let n = std::io::copy(&mut reader, &mut out)
        .with_context(|| format!("failed to restore {}", dest.display()))?;
    drop(out);
    copy_owner_and_mode(&meta, dest);
    copy_mtime(&meta, dest);
    Ok(n)
}

/// Decrypt the snapshot tree `src` into `dest` (created if missing).
pub fn decrypt_tree(src: &Path, dest: &Path) -> Result<()> {
    let mut dirs: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let out = dest.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&out)?;
            dirs.push((out, entry.metadata()?));
        } else if entry.file_type().is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            decrypt_file(entry.path(), &out)?;
        }
    }
    for (path, meta) in dirs.iter().rev() {
        copy_owner_and_mode(meta, path);
    }
    Ok(())
}

/// `decrypt_tree` on a blocking thread.
pub async fn decrypt_tree_async(src: PathBuf, dest: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || decrypt_tree(&src, &dest))
        .await
        .context("restore task failed")?
}

/// A temporary directory of decrypted snapshot files, removed when dropped.
#[derive(Debug)]
pub struct PlainDir {
    path: PathBuf,
}

impl PlainDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PlainDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Decrypt the files `rels` (relative to `base`, a directory of a local
/// snapshot) that exist into a new temporary directory, keeping their
/// relative paths. For dumps and metadata that have to be read or handed to
/// a container as plain files.
pub fn plain_subset(base: &Path, rels: &[&str]) -> Result<PlainDir> {
    let root = work_root();
    std::fs::create_dir_all(&root)?;
    let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    let dir = PlainDir {
        path: root.join(uuid::Uuid::new_v4().to_string()),
    };
    std::fs::create_dir_all(&dir.path)?;
    for rel in rels {
        let src = base.join(rel);
        if !src.is_file() {
            continue;
        }
        let dest = dir.path.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        decrypt_file(&src, &dest)?;
    }
    Ok(dir)
}

/// `plain_subset` on a blocking thread.
pub async fn plain_subset_async(base: PathBuf, rels: Vec<String>) -> Result<PlainDir> {
    tokio::task::spawn_blocking(move || {
        let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
        plain_subset(&base, &rels)
    })
    .await
    .context("restore task failed")?
}

/// Remove working directories a crash left behind (startup).
pub fn clean_work_dirs() {
    let _ = std::fs::remove_dir_all(work_root());
}

// ── Key rotation ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RotationReport {
    pub reencrypted: usize,
    pub unreadable: usize,
}

/// Re-encrypt with `current` every local snapshot file under `root` that
/// only `previous` decrypts. Idempotent; a crash midway leaves every file
/// readable (each one is replaced atomically) and the next start resumes.
/// Files hard-linked between snapshots are re-encrypted once and linked
/// again, so deduplication survives.
pub fn rotate_local_snapshots(
    root: &Path,
    current: &str,
    previous: &str,
) -> Result<RotationReport> {
    let mut report = RotationReport::default();
    if current == previous || !root.is_dir() {
        return Ok(report);
    }
    // (device, inode) of a rotated file → its new copy
    let mut rotated: HashMap<(u64, u64), PathBuf> = HashMap::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let inode = (meta.dev(), meta.ino());
        if let Some(new_copy) = rotated.get(&inode) {
            let tmp = sibling(path, ".rotating-");
            let _ = std::fs::remove_file(&tmp);
            std::fs::hard_link(new_copy, &tmp)?;
            std::fs::rename(&tmp, path)?;
            continue;
        }
        let Ok(file) = File::open(path) else { continue };
        let reader = match DecryptReader::open(file, &[current, previous], local_kind(), None) {
            Ok(r) if r.key_index() == 0 => continue,
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Key rotation: can't read {}: {e:#}", path.display());
                report.unreadable += 1;
                continue;
            }
        };
        let tmp = sibling(path, ".rotating-");
        let result = sa::encrypt_to_file(reader, &tmp, current, local_kind()).and_then(|_| {
            copy_owner_and_mode(&meta, &tmp);
            copy_mtime(&meta, &tmp);
            std::fs::rename(&tmp, path)?;
            Ok(())
        });
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.context(format!("failed to re-encrypt {}", path.display())));
        }
        if meta.nlink() > 1 {
            rotated.insert(inode, path.to_path_buf());
        }
        report.reencrypted += 1;
    }
    Ok(report)
}

/// Startup entry point: with TOND_PREVIOUS_SECRET_KEY set, move the local
/// snapshot copies onto TOND_SECRET_KEY (on a blocking thread).
pub async fn rotate_at_startup() {
    let Some(previous) = secret_key::previous_secret() else {
        return;
    };
    let current = secret_key::current_secret();
    let root = data_dir().join("backups").join("snapshots");
    let result =
        tokio::task::spawn_blocking(move || rotate_local_snapshots(&root, &current, &previous))
            .await;
    match result {
        Ok(Ok(report)) => {
            if report.reencrypted > 0 {
                tracing::info!(
                    "Key rotation: re-encrypted {} local snapshot file(s) with the new key",
                    report.reencrypted
                );
            }
            if report.unreadable > 0 {
                tracing::error!(
                    "Key rotation: {} local snapshot file(s) decrypt with neither key",
                    report.unreadable
                );
            }
        }
        Ok(Err(e)) => tracing::error!("Key rotation of local snapshots failed: {e:#}"),
        Err(e) => tracing::error!("Key rotation of local snapshots failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    fn write(path: &Path, data: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir: {e}"));
        }
        std::fs::write(path, data).unwrap_or_else(|e| panic!("write: {e}"));
    }

    fn plaintext(path: &Path, secrets: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        DecryptReader::open(
            File::open(path).unwrap_or_else(|e| panic!("open: {e}")),
            secrets,
            local_kind(),
            None,
        )
        .unwrap_or_else(|e| panic!("decrypt {}: {e:#}", path.display()))
        .read_to_end(&mut out)
        .unwrap_or_else(|e| panic!("read: {e}"));
        out
    }

    #[test]
    fn copies_are_encrypted_deduplicated_and_restorable() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let src = tmp.path().join("src");
        write(&src.join("a.txt"), b"alpha contents");
        write(&src.join("sub/b.txt"), b"beta contents");
        std::fs::set_permissions(
            src.join("sub/b.txt"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap_or_else(|e| panic!("chmod: {e}"));
        std::os::unix::fs::symlink("/etc/passwd", src.join("leak"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));

        let one = tmp.path().join("one");
        let (delta, total) =
            encrypt_tree(&src, &one, None).unwrap_or_else(|e| panic!("copy: {e:#}"));
        assert_eq!((delta, total), (27, 27));
        let raw = std::fs::read(one.join("a.txt")).unwrap_or_default();
        assert!(raw.starts_with(sa::LOCAL_MAGIC), "encrypted at rest");
        assert!(!raw.windows(5).any(|w| w == b"alpha"));
        assert!(std::fs::symlink_metadata(one.join("leak")).is_err());

        // Unchanged files are hard-linked; a changed one is encrypted again.
        write(&src.join("a.txt"), b"alpha changed!");
        let two = tmp.path().join("two");
        let (delta, _) =
            encrypt_tree(&src, &two, Some(&one)).unwrap_or_else(|e| panic!("copy: {e:#}"));
        assert_eq!(delta, 14);
        let ino = |p: &Path| std::fs::metadata(p).map(|m| m.ino()).unwrap_or(0);
        assert_eq!(ino(&one.join("sub/b.txt")), ino(&two.join("sub/b.txt")));
        assert_ne!(ino(&one.join("a.txt")), ino(&two.join("a.txt")));

        let restored = tmp.path().join("restored");
        decrypt_tree(&two, &restored).unwrap_or_else(|e| panic!("restore: {e:#}"));
        assert_eq!(
            std::fs::read(restored.join("a.txt")).ok().as_deref(),
            Some(&b"alpha changed!"[..])
        );
        let mode = std::fs::metadata(restored.join("sub/b.txt"))
            .map(|m| m.mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o640, "modes come back");
    }

    #[test]
    fn rotation_reencrypts_once_and_keeps_hard_links() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let root = tmp.path().join("snapshots");
        let kind = local_kind();
        let one = root.join("one");
        std::fs::create_dir_all(&one).unwrap_or_else(|e| panic!("mkdir: {e}"));
        sa::encrypt_to_file(&b"shared"[..], &one.join("f"), "old-key", kind)
            .unwrap_or_else(|e| panic!("encrypt: {e:#}"));
        sa::encrypt_to_file(&b"current"[..], &one.join("g"), "new-key", kind)
            .unwrap_or_else(|e| panic!("encrypt: {e:#}"));
        let two = root.join("two");
        std::fs::create_dir_all(&two).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::fs::hard_link(one.join("f"), two.join("f")).unwrap_or_else(|e| panic!("link: {e}"));

        let report = rotate_local_snapshots(&root, "new-key", "old-key")
            .unwrap_or_else(|e| panic!("rotate: {e:#}"));
        assert_eq!(report.reencrypted, 1, "{report:?}");
        assert_eq!(plaintext(&one.join("f"), &["new-key"]), b"shared");
        assert_eq!(plaintext(&two.join("f"), &["new-key"]), b"shared");
        assert_eq!(plaintext(&one.join("g"), &["new-key"]), b"current");
        let ino = |p: &Path| std::fs::metadata(p).map(|m| m.ino()).unwrap_or(0);
        assert_eq!(ino(&one.join("f")), ino(&two.join("f")), "still one copy");

        let again = rotate_local_snapshots(&root, "new-key", "old-key")
            .unwrap_or_else(|e| panic!("rotate: {e:#}"));
        assert_eq!(again, RotationReport::default(), "idempotent");
    }
}
