// upload.rs — safe extraction of uploaded source archives (`tond push`)
//
// An upload is a tar archive (gzip-compressed or plain) of the user's working
// directory. tond-server runs as root, so extracting it with system `tar`
// would honour absolute paths, `..`, symlinks to /etc, device nodes and
// setuid bits. Archives are therefore extracted here, in Rust, entry by
// entry, while the upload streams in:
//
//   - each upload gets its own directory, `<data>/uploads/<random>` (0700,
//     created with create_dir so an existing path is never reused)
//   - paths must be relative and may not contain `..`; `.` components are
//     dropped
//   - an entry may not be written through a symlink: every parent directory
//     must be a real directory created by this extraction
//   - symlink targets must be relative and stay inside the upload directory;
//     after extraction every symlink is resolved and must land inside it
//   - hard links must name a regular file extracted earlier; the content is
//     copied (so it counts towards the size cap)
//   - character/block devices, FIFOs and sparse files are refused
//   - setuid, setgid and sticky bits are stripped; files are owned by root
//     (tond-server) regardless of the uid/gid in the archive
//   - the total decompressed size is capped (setting `upload_max_bytes`,
//     default 1 GiB), as are the compressed size (same cap) and the number of
//     entries (MAX_UPLOAD_ENTRIES)
//
//   ArchiveReader        a blocking `Read` fed from the async upload stream
//   extract_archive()    validate and extract one archive into a directory
//   create_upload_dir()  make a fresh random directory under <data>/uploads
//   clear_upload_dirs()  remove leftovers of a previous process (startup)

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use rand::Rng;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;

/// Smallest accepted `upload_max_bytes` setting (1 MiB).
pub const MIN_UPLOAD_MAX_BYTES: u64 = 1024 * 1024;

/// Largest accepted `upload_max_bytes` setting (64 GiB).
pub const MAX_UPLOAD_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Most entries (files, directories, links) one upload may contain.
pub const MAX_UPLOAD_ENTRIES: u64 = 100_000;

/// Directory under the data dir that holds in-progress uploads.
pub const UPLOADS_DIR: &str = "uploads";

/// Why an upload was refused.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The archive exceeds a size or entry-count limit.
    #[error("{0}")]
    TooLarge(String),
    /// The archive is malformed or contains something Tond refuses to extract.
    #[error("{0}")]
    Invalid(String),
    /// A local I/O failure unrelated to the archive's content.
    #[error("{0}")]
    Io(String),
}

fn invalid(msg: impl Into<String>) -> UploadError {
    UploadError::Invalid(msg.into())
}

/// Limits applied to one extraction.
#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    /// Max total bytes of file content written (and max compressed input).
    pub max_bytes: u64,
    /// Max number of archive entries.
    pub max_entries: u64,
}

impl ExtractLimits {
    pub fn with_max_bytes(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            max_entries: MAX_UPLOAD_ENTRIES,
        }
    }
}

/// What an extraction produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExtractStats {
    pub files: u64,
    pub bytes: u64,
}

/// Human-readable size for error messages (MiB/GiB, one decimal).
pub fn format_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= 1024.0 * MIB {
        format!("{:.1} GiB", b / (1024.0 * MIB))
    } else {
        format!("{:.1} MiB", b / MIB)
    }
}

fn too_large(max_bytes: u64) -> UploadError {
    UploadError::TooLarge(format!(
        "The upload is larger than this server's limit of {} once unpacked. \
         Add large files (build output, dependencies, data) to .tondignore, or ask an \
         admin to raise the upload limit in Settings.",
        format_size(max_bytes)
    ))
}

// ── Streaming input ──────────────────────────────────────────────────────────

/// A blocking reader over chunks sent from an async task. Used inside
/// `spawn_blocking` so the archive is extracted while it is still uploading,
/// without buffering it in memory or on disk. Counts the bytes read and fails
/// once more than `max_bytes` arrive.
pub struct ArchiveReader {
    rx: tokio::sync::mpsc::Receiver<io::Result<Bytes>>,
    current: Bytes,
    read: u64,
    max_bytes: u64,
}

impl ArchiveReader {
    pub fn new(rx: tokio::sync::mpsc::Receiver<io::Result<Bytes>>, max_bytes: u64) -> Self {
        Self {
            rx,
            current: Bytes::new(),
            read: 0,
            max_bytes,
        }
    }
}

/// Marker carried inside io::Error when the compressed stream exceeds the cap.
#[derive(Debug)]
struct CompressedTooLarge;

impl std::fmt::Display for CompressedTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("compressed upload exceeds the size limit")
    }
}

impl std::error::Error for CompressedTooLarge {}

impl Read for ArchiveReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(chunk)) => {
                    self.read += chunk.len() as u64;
                    if self.read > self.max_bytes {
                        return Err(io::Error::other(CompressedTooLarge));
                    }
                    self.current = chunk;
                }
                Some(Err(e)) => return Err(e),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current[..n]);
        let _ = self.current.split_to(n);
        Ok(n)
    }
}

// ── Paths ────────────────────────────────────────────────────────────────────

/// Normalise an archive path: relative, no `..`, `.` dropped. `None` for an
/// empty path (e.g. the `./` root entry).
fn safe_relative_path(raw: &Path) -> Result<Option<PathBuf>, UploadError> {
    let mut out = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(invalid(format!(
                    "'{}' contains '..'; archives may only contain paths inside the project",
                    raw.display()
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid(format!(
                    "'{}' is an absolute path; archives may only contain relative paths",
                    raw.display()
                )));
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// Lexically resolve a symlink target relative to the link's directory
/// (both relative to the extraction root). `None` when it leaves the root.
fn resolve_link_target(link_rel: &Path, target: &Path) -> Option<PathBuf> {
    if target.is_absolute() {
        return None;
    }
    let mut parts: Vec<std::ffi::OsString> = link_rel
        .parent()
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_os_string())
                .collect()
        })
        .unwrap_or_default();
    for component in target.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.iter().collect())
}

/// Make sure every ancestor of `rel` below `root` is a real directory
/// (creating missing ones), never a symlink or file.
fn ensure_parent_dirs(root: &Path, rel: &Path) -> Result<(), UploadError> {
    let Some(parent) = rel.parent() else {
        return Ok(());
    };
    let mut current = root.to_path_buf();
    for component in parent.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => {
                return Err(invalid(format!(
                    "'{}' is inside '{}', which is not a directory in the archive",
                    rel.display(),
                    current.strip_prefix(root).unwrap_or(&current).display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .mode(0o755)
                    .create(&current)
                    .map_err(|e| UploadError::Io(format!("could not create directory: {e}")))?;
            }
            Err(e) => return Err(UploadError::Io(format!("could not inspect path: {e}"))),
        }
    }
    Ok(())
}

/// Remove whatever an earlier entry left at `path` (later entries win, like
/// tar), refusing to replace a directory with a non-directory.
fn clear_existing(path: &Path, rel: &Path) -> Result<(), UploadError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Err(invalid(format!(
            "'{}' appears in the archive both as a directory and as a file",
            rel.display()
        ))),
        Ok(_) => fs::remove_file(path)
            .map_err(|e| UploadError::Io(format!("could not replace '{}': {e}", rel.display()))),
        Err(_) => Ok(()),
    }
}

/// File permission bits from the archive with setuid/setgid/sticky removed,
/// always readable and writable by the owner.
fn sanitized_mode(mode: u32) -> u32 {
    (mode & 0o777) | 0o600
}

// ── Extraction ───────────────────────────────────────────────────────────────

/// Detect gzip vs. plain tar from the first bytes and wrap accordingly.
fn decompressed<R: Read + 'static>(reader: R) -> Result<Box<dyn Read>, UploadError> {
    let mut buffered = BufReader::with_capacity(64 * 1024, reader);
    let head = buffered.fill_buf().map_err(map_read_error)?;
    if head.starts_with(&[0x1f, 0x8b]) {
        Ok(Box::new(flate2::bufread::MultiGzDecoder::new(buffered)))
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Err(invalid(
            "zstd-compressed archives aren't supported; upload a .tar.gz or plain .tar",
        ))
    } else if head.is_empty() {
        Err(invalid("The upload is empty"))
    } else {
        Ok(Box::new(buffered))
    }
}

fn map_read_error(e: io::Error) -> UploadError {
    if e.get_ref()
        .is_some_and(|inner| inner.is::<CompressedTooLarge>())
    {
        // The caller replaces this with the configured limit in the message.
        return UploadError::TooLarge(String::new());
    }
    invalid(format!("The upload is not a valid .tar.gz archive: {e}"))
}

/// Validate and extract a tar (optionally gzip) archive into `dest`, which
/// must be an empty directory. On error `dest` may hold a partial extraction;
/// the caller removes it.
pub fn extract_archive<R: Read + 'static>(
    reader: R,
    dest: &Path,
    limits: ExtractLimits,
) -> Result<ExtractStats, UploadError> {
    let result = extract_inner(reader, dest, limits);
    match result {
        Err(UploadError::TooLarge(msg)) if msg.is_empty() => Err(too_large(limits.max_bytes)),
        other => other,
    }
}

fn extract_inner<R: Read + 'static>(
    reader: R,
    dest: &Path,
    limits: ExtractLimits,
) -> Result<ExtractStats, UploadError> {
    let mut archive = tar::Archive::new(decompressed(reader)?);
    let mut stats = ExtractStats::default();
    let mut entries_seen: u64 = 0;
    let mut symlinks: Vec<PathBuf> = Vec::new();

    for entry in archive.entries().map_err(map_read_error)? {
        let mut entry = entry.map_err(map_read_error)?;
        entries_seen += 1;
        if entries_seen > limits.max_entries {
            return Err(UploadError::TooLarge(format!(
                "The upload has more than {} files. Add dependency and build directories \
                 to .tondignore.",
                limits.max_entries
            )));
        }

        let entry_type = entry.header().entry_type();
        // PAX global headers carry metadata only.
        if entry_type.is_pax_global_extensions() {
            continue;
        }
        let raw_path = entry.path().map_err(map_read_error)?.into_owned();
        let Some(rel) = safe_relative_path(&raw_path)? else {
            continue;
        };
        let target = dest.join(&rel);
        let mode = entry.header().mode().unwrap_or(0o644);

        if entry_type.is_dir() {
            ensure_parent_dirs(dest, &rel)?;
            match fs::symlink_metadata(&target) {
                Ok(meta) if meta.file_type().is_dir() => {}
                Ok(_) => {
                    return Err(invalid(format!(
                        "'{}' appears in the archive both as a file and as a directory",
                        rel.display()
                    )));
                }
                Err(_) => fs::DirBuilder::new()
                    .mode(0o755)
                    .create(&target)
                    .map_err(|e| UploadError::Io(format!("could not create directory: {e}")))?,
            }
            let _ = fs::set_permissions(
                &target,
                fs::Permissions::from_mode(sanitized_mode(mode) | 0o700),
            );
        } else if entry_type.is_file() || entry_type == tar::EntryType::Continuous {
            let declared = entry.header().size().map_err(map_read_error)?;
            if stats.bytes.saturating_add(declared) > limits.max_bytes {
                return Err(too_large(limits.max_bytes));
            }
            ensure_parent_dirs(dest, &rel)?;
            clear_existing(&target, &rel)?;
            let written = write_file(&mut entry, &target, mode, limits.max_bytes - stats.bytes)
                .map_err(|e| match e {
                    UploadError::TooLarge(_) => too_large(limits.max_bytes),
                    other => other,
                })?;
            stats.bytes += written;
            stats.files += 1;
        } else if entry_type.is_symlink() {
            let link_target = entry
                .link_name()
                .map_err(map_read_error)?
                .ok_or_else(|| invalid(format!("symlink '{}' has no target", rel.display())))?
                .into_owned();
            if resolve_link_target(&rel, &link_target).is_none() {
                return Err(invalid(format!(
                    "symlink '{}' points to '{}', outside the project; archives may only \
                     link to files inside the project",
                    rel.display(),
                    link_target.display()
                )));
            }
            ensure_parent_dirs(dest, &rel)?;
            clear_existing(&target, &rel)?;
            std::os::unix::fs::symlink(&link_target, &target)
                .map_err(|e| UploadError::Io(format!("could not create symlink: {e}")))?;
            symlinks.push(rel);
        } else if entry_type.is_hard_link() {
            let raw_target = entry
                .link_name()
                .map_err(map_read_error)?
                .ok_or_else(|| invalid(format!("hard link '{}' has no target", rel.display())))?
                .into_owned();
            let source_rel = safe_relative_path(&raw_target)?.ok_or_else(|| {
                invalid(format!("hard link '{}' has an empty target", rel.display()))
            })?;
            let source = dest.join(&source_rel);
            // Every ancestor was created by this extraction as a real
            // directory, so symlink_metadata on the leaf is enough.
            let is_file = fs::symlink_metadata(&source)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false);
            if !is_file {
                return Err(invalid(format!(
                    "hard link '{}' points to '{}', which is not a file earlier in the archive",
                    rel.display(),
                    raw_target.display()
                )));
            }
            ensure_parent_dirs(dest, &rel)?;
            clear_existing(&target, &rel)?;
            let mut src = fs::File::open(&source)
                .map_err(|e| UploadError::Io(format!("could not read link source: {e}")))?;
            let src_mode = src
                .metadata()
                .map(|m| m.permissions().mode())
                .unwrap_or(0o644);
            let written = write_file(&mut src, &target, src_mode, limits.max_bytes - stats.bytes)
                .map_err(|e| match e {
                UploadError::TooLarge(_) => too_large(limits.max_bytes),
                other => other,
            })?;
            stats.bytes += written;
            stats.files += 1;
        } else if entry_type.is_character_special() || entry_type.is_block_special() {
            return Err(invalid(format!(
                "'{}' is a device file; archives may not contain devices",
                rel.display()
            )));
        } else if entry_type.is_fifo() {
            return Err(invalid(format!(
                "'{}' is a named pipe (FIFO); archives may not contain pipes",
                rel.display()
            )));
        } else if entry_type.is_gnu_longname()
            || entry_type.is_gnu_longlink()
            || entry_type.is_pax_local_extensions()
        {
            // Consumed by the tar crate; never yielded, but harmless if so.
            continue;
        } else {
            return Err(invalid(format!(
                "'{}' has an unsupported entry type ({:?}); archives may only contain \
                 files, directories and links",
                rel.display(),
                entry_type
            )));
        }
    }

    verify_symlinks(dest, &symlinks)?;
    Ok(stats)
}

/// Write `reader` into a new file at `path` with sanitised permissions,
/// failing once more than `remaining` bytes are written.
fn write_file(
    reader: &mut dyn Read,
    path: &Path,
    mode: u32,
    remaining: u64,
) -> Result<u64, UploadError> {
    // create_new = O_CREAT|O_EXCL, which also fails when the leaf is a
    // (dangling) symlink, so the write can never follow a link.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| UploadError::Io(format!("could not create file: {e}")))?;
    let mut limited = reader.take(remaining.saturating_add(1));
    let written = io::copy(&mut limited, &mut file).map_err(map_read_error)?;
    if written > remaining {
        return Err(UploadError::TooLarge(String::new()));
    }
    file.flush()
        .map_err(|e| UploadError::Io(format!("could not write file: {e}")))?;
    // Set the final mode explicitly: the umask must not add bits, and the
    // archive's setuid/setgid/sticky bits are dropped.
    file.set_permissions(fs::Permissions::from_mode(sanitized_mode(mode)))
        .map_err(|e| UploadError::Io(format!("could not set permissions: {e}")))?;
    Ok(written)
}

/// Resolve every extracted symlink and require it to land inside `dest`.
/// Catches chains that stay inside lexically but escape through another
/// link (`b -> ..` inside a subdirectory, then `a -> sub/b/..`), and refuses
/// links to files that aren't in the archive.
fn verify_symlinks(dest: &Path, symlinks: &[PathBuf]) -> Result<(), UploadError> {
    if symlinks.is_empty() {
        return Ok(());
    }
    let root = fs::canonicalize(dest)
        .map_err(|e| UploadError::Io(format!("could not resolve upload directory: {e}")))?;
    for rel in symlinks {
        let link = dest.join(rel);
        let target = fs::read_link(&link).unwrap_or_default();
        match fs::canonicalize(&link) {
            Ok(resolved) if resolved.starts_with(&root) => {}
            Ok(_) => {
                return Err(invalid(format!(
                    "symlink '{}' points to '{}', which resolves outside the project",
                    rel.display(),
                    target.display()
                )));
            }
            Err(_) => {
                return Err(invalid(format!(
                    "symlink '{}' points to '{}', which isn't in the archive; remove the link \
                     or add it to .tondignore",
                    rel.display(),
                    target.display()
                )));
            }
        }
    }
    Ok(())
}

// ── Upload directories ───────────────────────────────────────────────────────

/// `<data>/uploads`
pub fn uploads_root(data_dir: &Path) -> PathBuf {
    data_dir.join(UPLOADS_DIR)
}

/// Create `<data>/uploads/<random>` (0700). The name is 128 random bits, and
/// `create_dir` fails rather than reuse an existing path.
pub fn create_upload_dir(data_dir: &Path) -> io::Result<PathBuf> {
    let root = uploads_root(data_dir);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let mut name_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut name_bytes);
    let name = hex::encode(name_bytes);
    let dir = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Remove upload directories left behind by a previous process.
pub fn clear_upload_dirs(data_dir: &Path) {
    let root = uploads_root(data_dir);
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let _ = if is_dir {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use flate2::{Compression, write::GzEncoder};
    use tempfile::TempDir;

    fn limits() -> ExtractLimits {
        ExtractLimits {
            max_bytes: 1024 * 1024,
            max_entries: 100,
        }
    }

    /// Build a tar archive with raw headers so paths and types that the
    /// `tar` builder refuses (absolute, `..`, devices) can be written.
    struct RawTar {
        builder: tar::Builder<Vec<u8>>,
    }

    impl RawTar {
        fn new() -> Self {
            Self {
                builder: tar::Builder::new(Vec::new()),
            }
        }

        fn header(path: &str, entry_type: tar::EntryType, size: u64, mode: u32) -> tar::Header {
            let mut header = tar::Header::new_gnu();
            {
                let gnu = header.as_gnu_mut().unwrap_or_else(|| panic!("gnu header"));
                let bytes = path.as_bytes();
                gnu.name[..bytes.len()].copy_from_slice(bytes);
            }
            header.set_entry_type(entry_type);
            header.set_size(size);
            header.set_mode(mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            header
        }

        fn file(mut self, path: &str, content: &[u8], mode: u32) -> Self {
            let header = Self::header(path, tar::EntryType::Regular, content.len() as u64, mode);
            self.builder
                .append(&header, content)
                .unwrap_or_else(|e| panic!("append file: {e}"));
            self
        }

        fn dir(mut self, path: &str) -> Self {
            let header = Self::header(path, tar::EntryType::Directory, 0, 0o755);
            self.builder
                .append(&header, io::empty())
                .unwrap_or_else(|e| panic!("append dir: {e}"));
            self
        }

        fn link(mut self, path: &str, entry_type: tar::EntryType, target: &str) -> Self {
            let mut header = Self::header(path, entry_type, 0, 0o777);
            {
                let gnu = header.as_gnu_mut().unwrap_or_else(|| panic!("gnu header"));
                let bytes = target.as_bytes();
                gnu.linkname[..bytes.len()].copy_from_slice(bytes);
            }
            header.set_cksum();
            self.builder
                .append(&header, io::empty())
                .unwrap_or_else(|e| panic!("append link: {e}"));
            self
        }

        fn special(mut self, path: &str, entry_type: tar::EntryType) -> Self {
            let header = Self::header(path, entry_type, 0, 0o644);
            self.builder
                .append(&header, io::empty())
                .unwrap_or_else(|e| panic!("append special: {e}"));
            self
        }

        fn tar(self) -> Vec<u8> {
            self.builder
                .into_inner()
                .unwrap_or_else(|e| panic!("finish tar: {e}"))
        }

        fn gz(self) -> Vec<u8> {
            let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
            enc.write_all(&self.tar())
                .unwrap_or_else(|e| panic!("gzip: {e}"));
            enc.finish().unwrap_or_else(|e| panic!("gzip finish: {e}"))
        }
    }

    fn extract(
        bytes: Vec<u8>,
        limits: ExtractLimits,
    ) -> (TempDir, Result<ExtractStats, UploadError>) {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let result = extract_archive(Cursor::new(bytes), &dest, limits);
        (dir, result)
    }

    fn expect_invalid(result: Result<ExtractStats, UploadError>, needle: &str) {
        match result {
            Err(UploadError::Invalid(msg)) => assert!(
                msg.contains(needle),
                "error should mention {needle:?}, got {msg:?}"
            ),
            other => panic!("expected Invalid containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn extracts_a_normal_gzip_archive_with_files_dirs_and_internal_links() {
        let archive = RawTar::new()
            .dir("./")
            .dir("./src/")
            .file("./src/main.rs", b"fn main() {}", 0o644)
            .file("./run.sh", b"#!/bin/sh\n", 0o755)
            .link("./latest", tar::EntryType::Symlink, "src/main.rs")
            .link("./src/copy.rs", tar::EntryType::Link, "src/main.rs")
            .gz();
        let (dir, result) = extract(archive, limits());
        let stats = result.unwrap_or_else(|e| panic!("normal archive should extract: {e}"));
        let out = dir.path().join("out");
        assert_eq!(
            stats.files, 3,
            "three files (one via hard link) should be counted"
        );
        assert_eq!(
            fs::read_to_string(out.join("src/copy.rs")).unwrap_or_default(),
            "fn main() {}",
            "a hard link should become a copy of its source"
        );
        assert_eq!(
            fs::read_link(out.join("latest")).unwrap_or_default(),
            PathBuf::from("src/main.rs"),
            "an internal symlink should be kept"
        );
        let mode = fs::metadata(out.join("run.sh"))
            .map(|m| m.permissions().mode() & 0o7777)
            .unwrap_or(0);
        assert_eq!(mode, 0o755, "executable bits should be preserved");
    }

    #[test]
    fn extracts_a_plain_uncompressed_tar() {
        let archive = RawTar::new().file("a.txt", b"hi", 0o644).tar();
        let (dir, result) = extract(archive, limits());
        assert!(result.is_ok(), "plain tar should extract: {result:?}");
        assert!(dir.path().join("out/a.txt").is_file());
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_d, r) = extract(
            RawTar::new().file("/etc/cron.d/x", b"x", 0o644).gz(),
            limits(),
        );
        expect_invalid(r, "absolute path");
    }

    #[test]
    fn rejects_parent_dir_components() {
        let (dir, r) = extract(
            RawTar::new().file("a/../../escape", b"x", 0o644).gz(),
            limits(),
        );
        expect_invalid(r, "'..'");
        assert!(
            !dir.path().join("escape").exists(),
            "nothing may be written outside"
        );
    }

    #[test]
    fn rejects_symlinks_pointing_outside() {
        let (_d, r) = extract(
            RawTar::new()
                .link("etc", tar::EntryType::Symlink, "/etc")
                .gz(),
            limits(),
        );
        expect_invalid(r, "outside the project");
        let (_d, r) = extract(
            RawTar::new()
                .link("up", tar::EntryType::Symlink, "../../x")
                .gz(),
            limits(),
        );
        expect_invalid(r, "outside the project");
    }

    #[test]
    fn rejects_writing_through_a_symlink() {
        let archive = RawTar::new()
            .dir("sub/")
            .link("alias", tar::EntryType::Symlink, "sub")
            .file("alias/evil", b"x", 0o644)
            .gz();
        let (_d, r) = extract(archive, limits());
        expect_invalid(r, "not a directory");
    }

    #[test]
    fn rejects_symlink_chains_that_escape_through_another_link() {
        // sub/b -> .. resolves to the root (fine lexically); a -> sub/b/..
        // is lexically `sub` but really the root's parent.
        let archive = RawTar::new()
            .dir("sub/")
            .link("sub/b", tar::EntryType::Symlink, "..")
            .link("a", tar::EntryType::Symlink, "sub/b/..")
            .gz();
        let (_d, r) = extract(archive, limits());
        expect_invalid(r, "outside the project");
    }

    #[test]
    fn rejects_dangling_symlinks() {
        let (_d, r) = extract(
            RawTar::new()
                .link("gone", tar::EntryType::Symlink, "missing.txt")
                .gz(),
            limits(),
        );
        expect_invalid(r, "isn't in the archive");
    }

    #[test]
    fn rejects_hard_links_outside_or_to_missing_files() {
        let (_d, r) = extract(
            RawTar::new()
                .link("shadow", tar::EntryType::Link, "/etc/shadow")
                .gz(),
            limits(),
        );
        expect_invalid(r, "absolute path");
        let (_d, r) = extract(
            RawTar::new().link("x", tar::EntryType::Link, "nope").gz(),
            limits(),
        );
        expect_invalid(r, "not a file earlier");
    }

    #[test]
    fn rejects_devices_and_fifos() {
        let (_d, r) = extract(
            RawTar::new().special("null", tar::EntryType::Char).gz(),
            limits(),
        );
        expect_invalid(r, "device");
        let (_d, r) = extract(
            RawTar::new().special("disk", tar::EntryType::Block).gz(),
            limits(),
        );
        expect_invalid(r, "device");
        let (_d, r) = extract(
            RawTar::new().special("pipe", tar::EntryType::Fifo).gz(),
            limits(),
        );
        expect_invalid(r, "FIFO");
    }

    #[test]
    fn strips_setuid_setgid_and_sticky_bits() {
        let archive = RawTar::new()
            .file("suid", b"#!/bin/sh\n", 0o6755)
            .file("sticky", b"x", 0o1644)
            .gz();
        let (dir, result) = extract(archive, limits());
        assert!(result.is_ok(), "archive should extract: {result:?}");
        for (name, expected) in [("suid", 0o755), ("sticky", 0o644)] {
            let mode = fs::metadata(dir.path().join("out").join(name))
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0);
            assert_eq!(mode, expected, "{name} should lose its special bits");
        }
    }

    #[test]
    fn caps_the_total_decompressed_size() {
        // 3 × 400 KiB of zeros compresses to almost nothing but unpacks past 1 MiB.
        let chunk = vec![0u8; 400 * 1024];
        let archive = RawTar::new()
            .file("a", &chunk, 0o644)
            .file("b", &chunk, 0o644)
            .file("c", &chunk, 0o644)
            .gz();
        assert!(
            archive.len() < 64 * 1024,
            "the bomb should be small compressed"
        );
        let (_d, r) = extract(archive, limits());
        match r {
            Err(UploadError::TooLarge(msg)) => {
                assert!(
                    msg.contains("1.0 MiB"),
                    "message should name the limit: {msg}"
                )
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn caps_hard_link_copies_towards_the_size_limit() {
        let chunk = vec![0u8; 600 * 1024];
        let archive = RawTar::new()
            .file("big", &chunk, 0o644)
            .link("big2", tar::EntryType::Link, "big")
            .gz();
        let (_d, r) = extract(archive, limits());
        assert!(
            matches!(r, Err(UploadError::TooLarge(_))),
            "hard-link copies must count towards the cap: {r:?}"
        );
    }

    #[test]
    fn caps_the_entry_count() {
        let mut t = RawTar::new();
        for i in 0..5 {
            t = t.file(&format!("f{i}"), b"x", 0o644);
        }
        let (_d, r) = extract(
            t.gz(),
            ExtractLimits {
                max_bytes: 1024 * 1024,
                max_entries: 3,
            },
        );
        assert!(
            matches!(r, Err(UploadError::TooLarge(ref m)) if m.contains("more than 3 files")),
            "entry cap should trigger: {r:?}"
        );
    }

    #[test]
    fn rejects_garbage_and_empty_uploads() {
        let (_d, r) = extract(Vec::new(), limits());
        expect_invalid(r, "empty");
        let (_d, r) = extract(vec![0x1f, 0x8b, 1, 2, 3], limits());
        expect_invalid(r, "not a valid");
        let (_d, r) = extract(vec![0x28, 0xb5, 0x2f, 0xfd, 0], limits());
        expect_invalid(r, "zstd");
    }

    #[test]
    fn a_file_replacing_a_directory_is_refused() {
        let archive = RawTar::new().dir("x/").file("x", b"boom", 0o644).gz();
        let (_d, r) = extract(archive, limits());
        expect_invalid(r, "both as a directory and as a file");
    }

    #[tokio::test]
    async fn archive_reader_streams_chunks_and_caps_compressed_size() {
        let archive = RawTar::new().file("a.txt", b"hello", 0o644).gz();
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let dest_clone = dest.clone();
        let job = tokio::task::spawn_blocking(move || {
            extract_archive(ArchiveReader::new(rx, 1024 * 1024), &dest_clone, limits())
        });
        for chunk in archive.chunks(7) {
            // The extractor stops at the tar end-of-archive marker and may
            // drop the receiver before the gzip trailer arrives.
            if tx.send(Ok(Bytes::copy_from_slice(chunk))).await.is_err() {
                break;
            }
        }
        drop(tx);
        let result = job.await.unwrap_or_else(|e| panic!("join: {e}"));
        assert!(
            result.is_ok(),
            "streamed archive should extract: {result:?}"
        );
        assert_eq!(
            fs::read_to_string(dest.join("a.txt")).unwrap_or_default(),
            "hello"
        );

        // Compressed size over the cap.
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let dest2 = dir.path().join("out2");
        fs::create_dir(&dest2).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let job = tokio::task::spawn_blocking(move || {
            extract_archive(
                ArchiveReader::new(rx, 10),
                &dest2,
                ExtractLimits {
                    max_bytes: 10,
                    max_entries: 10,
                },
            )
        });
        let big = RawTar::new().file("a", &[7u8; 4096], 0o644).tar();
        for chunk in big.chunks(512) {
            if tx.send(Ok(Bytes::copy_from_slice(chunk))).await.is_err() {
                break;
            }
        }
        drop(tx);
        let result = job.await.unwrap_or_else(|e| panic!("join: {e}"));
        assert!(
            matches!(result, Err(UploadError::TooLarge(ref m)) if !m.is_empty()),
            "compressed cap should give a TooLarge with a message: {result:?}"
        );
    }

    #[test]
    fn upload_dirs_are_random_private_and_cleared() {
        let data = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let a = create_upload_dir(data.path()).unwrap_or_else(|e| panic!("create: {e}"));
        let b = create_upload_dir(data.path()).unwrap_or_else(|e| panic!("create: {e}"));
        assert_ne!(a, b, "each upload gets its own directory");
        assert!(a.starts_with(uploads_root(data.path())));
        let mode = fs::metadata(&a)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o700, "upload dirs are private");
        clear_upload_dirs(data.path());
        assert!(
            !a.exists() && !b.exists(),
            "startup cleanup removes leftovers"
        );
    }

    #[test]
    fn resolve_link_target_handles_relative_paths() {
        assert_eq!(
            resolve_link_target(Path::new("a/b/link"), Path::new("../c")),
            Some(PathBuf::from("a/c"))
        );
        assert_eq!(
            resolve_link_target(Path::new("link"), Path::new("..")),
            None
        );
        assert_eq!(
            resolve_link_target(Path::new("link"), Path::new("/abs")),
            None
        );
    }
}
