// snapshot_archive.rs — streaming, encrypted, authenticated snapshot archives
//
// A snapshot is uploaded to remote storage as one archive, built and read as
// a stream so memory stays constant no matter how big the data is:
//
//   directory ─tar─▶ gzip ─▶ chunked AES-256-GCM ─▶ SHA-256 ─▶ sink (S3 parts, file)
//
// Format ("TONDSNP1"):
//
//   header  = magic "TONDSNP1" | version u8 (1) | chunk_size u32 BE
//             | salt [16] | nonce_prefix [7] | id_len u16 BE | snapshot id
//   records = AES-256-GCM(chunk) || tag[16], one per `chunk_size` plaintext
//             bytes; the last record may be shorter (or empty)
//
// - key: HKDF-SHA256(TOND_SECRET_KEY, salt, "tond snapshot archive v1"), so
//   every archive has its own key and none is the stored-secrets key;
// - nonce of record i: nonce_prefix || i (u32 BE) || last-flag (0/1), the
//   STREAM construction: records can't be reordered, dropped or replayed, and
//   a truncated archive fails because its new last record wasn't sealed as
//   last;
// - the whole header is each record's associated data, so the header (and
//   the snapshot id it names) is authenticated too.
//
// Restores verify the whole archive (`verify_archive`) before anything is
// extracted, then extract (`extract_archive`) accepting only regular files
// and directories with relative, non-escaping paths.
//
// This file depends only on external crates so the memory-ceiling test in
// tests/snapshot_stream_memory.rs can include it with its own allocator.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};
use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"TONDSNP1";
pub const VERSION: u8 = 1;
/// Plaintext bytes per record.
pub const CHUNK_SIZE: usize = 64 * 1024;
const TAG_LEN: usize = 16;
const SALT_LEN: usize = 16;
const NONCE_PREFIX_LEN: usize = 7;
const MAX_ID_LEN: usize = 256;
const KEY_INFO: &[u8] = b"tond snapshot archive v1";
/// gzip level: snapshot data is often already compressed; level 3 keeps CPU
/// low on a small VPS while still shrinking text (SQL dumps, the database).
const GZIP_LEVEL: u32 = 3;

/// What `write_archive` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSummary {
    /// Archive size in bytes (header + records)
    pub bytes: u64,
    /// SHA-256 of the archive bytes, hex
    pub sha256: String,
}

// ── Key derivation ───────────────────────────────────────────────────────────

fn derive_key(secret: &str, salt: &[u8]) -> [u8; 32] {
    let mut extract =
        <Hmac<Sha256> as KeyInit>::new_from_slice(salt).expect("HMAC accepts any key length");
    extract.update(secret.as_bytes());
    let prk = extract.finalize().into_bytes();
    let mut expand =
        <Hmac<Sha256> as KeyInit>::new_from_slice(&prk).expect("HMAC accepts any key length");
    expand.update(KEY_INFO);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

// ── Header ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub chunk_size: usize,
    pub salt: [u8; SALT_LEN],
    pub nonce_prefix: [u8; NONCE_PREFIX_LEN],
    pub snapshot_id: String,
}

impl Header {
    fn new(snapshot_id: &str) -> Result<Self> {
        ensure!(
            snapshot_id.len() <= MAX_ID_LEN,
            "snapshot id is too long for the archive header"
        );
        let mut salt = [0u8; SALT_LEN];
        let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut salt);
        rng.fill_bytes(&mut nonce_prefix);
        Ok(Self {
            chunk_size: CHUNK_SIZE,
            salt,
            nonce_prefix,
            snapshot_id: snapshot_id.to_string(),
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 1 + 4 + SALT_LEN + NONCE_PREFIX_LEN + 2 + 64);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&(self.chunk_size as u32).to_be_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce_prefix);
        out.extend_from_slice(&(self.snapshot_id.len() as u16).to_be_bytes());
        out.extend_from_slice(self.snapshot_id.as_bytes());
        out
    }

    /// Read and parse a header, returning it with its raw bytes (the AAD).
    fn read<R: Read>(input: &mut R) -> Result<(Self, Vec<u8>)> {
        let mut fixed = [0u8; 8 + 1 + 4 + SALT_LEN + NONCE_PREFIX_LEN + 2];
        input
            .read_exact(&mut fixed)
            .context("not a Tond snapshot archive (too short)")?;
        ensure!(
            &fixed[..8] == MAGIC,
            "not a Tond snapshot archive (bad magic)"
        );
        ensure!(
            fixed[8] == VERSION,
            "unsupported snapshot archive version {}",
            fixed[8]
        );
        let chunk_size = u32::from_be_bytes(fixed[9..13].try_into().expect("4 bytes")) as usize;
        ensure!(
            (1024..=16 * 1024 * 1024).contains(&chunk_size),
            "invalid chunk size in snapshot archive"
        );
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&fixed[13..13 + SALT_LEN]);
        let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
        nonce_prefix.copy_from_slice(&fixed[13 + SALT_LEN..13 + SALT_LEN + NONCE_PREFIX_LEN]);
        let id_len = u16::from_be_bytes(
            fixed[fixed.len() - 2..]
                .try_into()
                .expect("2 bytes for the id length"),
        ) as usize;
        ensure!(
            id_len <= MAX_ID_LEN,
            "invalid snapshot id in archive header"
        );
        let mut id = vec![0u8; id_len];
        input
            .read_exact(&mut id)
            .context("snapshot archive header is truncated")?;
        let snapshot_id =
            String::from_utf8(id).context("snapshot id in archive header is not UTF-8")?;
        let mut raw = fixed.to_vec();
        raw.extend_from_slice(snapshot_id.as_bytes());
        Ok((
            Self {
                chunk_size,
                salt,
                nonce_prefix,
                snapshot_id,
            },
            raw,
        ))
    }
}

fn nonce_for(prefix: &[u8; NONCE_PREFIX_LEN], counter: u32, last: bool) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    nonce[NONCE_PREFIX_LEN..11].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = u8::from(last);
    Nonce::assume_unique_for_key(nonce)
}

/// AES-256-GCM (aws-lc) under the archive key derived from `secret`.
fn cipher_for(secret: &str, salt: &[u8]) -> LessSafeKey {
    let key = derive_key(secret, salt);
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).expect("a 32-byte AES-256 key"))
}

// ── Writing ──────────────────────────────────────────────────────────────────

/// Counts and hashes everything written through it.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Encrypts a byte stream into records. `finish` seals the last record.
struct EncryptWriter<W: Write> {
    inner: W,
    cipher: LessSafeKey,
    header: Vec<u8>,
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    counter: u32,
    buf: Vec<u8>,
    chunk_size: usize,
}

impl<W: Write> EncryptWriter<W> {
    fn new(mut inner: W, secret: &str, snapshot_id: &str) -> Result<Self> {
        let header = Header::new(snapshot_id)?;
        let raw = header.encode();
        inner.write_all(&raw)?;
        Ok(Self {
            inner,
            cipher: cipher_for(secret, &header.salt),
            header: raw,
            nonce_prefix: header.nonce_prefix,
            counter: 0,
            buf: Vec::with_capacity(header.chunk_size + TAG_LEN),
            chunk_size: header.chunk_size,
        })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let nonce = nonce_for(&self.nonce_prefix, self.counter, last);
        let tag = self
            .cipher
            .seal_in_place_separate_tag(nonce, Aad::from(&self.header), &mut self.buf)
            .map_err(|_| io::Error::other("snapshot encryption failed"))?;
        self.inner.write_all(&self.buf)?;
        self.inner.write_all(tag.as_ref())?;
        self.buf.clear();
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("snapshot archive is too large"))?;
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        // Seal a full chunk only once more data arrives, so the final record
        // (sealed by `finish`) is never missing.
        if self.buf.len() == self.chunk_size {
            self.seal(false)?;
        }
        let room = self.chunk_size - self.buf.len();
        let n = room.min(data.len());
        self.buf.extend_from_slice(&data[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Archive `src_dir` (regular files and directories only; symlinks, devices
/// and sockets are skipped) into `out`, encrypted with a key derived from
/// `secret` and bound to `snapshot_id`.
pub fn write_archive<W: Write>(
    src_dir: &Path,
    out: W,
    secret: &str,
    snapshot_id: &str,
) -> Result<(ArchiveSummary, W)> {
    let hashing = HashingWriter {
        inner: out,
        hasher: Sha256::new(),
        bytes: 0,
    };
    let encrypt = EncryptWriter::new(hashing, secret, snapshot_id)?;
    let gzip = GzEncoder::new(encrypt, Compression::new(GZIP_LEVEL));
    let mut tar = tar::Builder::new(gzip);
    tar.follow_symlinks(false);
    tar.mode(tar::HeaderMode::Complete);

    for entry in walkdir::WalkDir::new(src_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.context("failed to walk the snapshot directory")?;
        let rel = entry
            .path()
            .strip_prefix(src_dir)
            .context("walked outside the snapshot directory")?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let file_type = entry.file_type();
        if file_type.is_dir() {
            tar.append_dir(rel, entry.path())
                .with_context(|| format!("failed to archive {}", rel.display()))?;
        } else if file_type.is_file() {
            let mut file = File::open(entry.path())
                .with_context(|| format!("failed to open {}", rel.display()))?;
            tar.append_file(rel, &mut file)
                .with_context(|| format!("failed to archive {}", rel.display()))?;
        }
        // Anything else (symlinks, sockets, devices) is not snapshot data.
    }

    let gzip = tar
        .into_inner()
        .context("failed to finish the tar stream")?;
    let encrypt = gzip.finish().context("failed to finish compression")?;
    let hashing = encrypt
        .finish()
        .context("failed to seal the last archive record")?;
    let summary = ArchiveSummary {
        bytes: hashing.bytes,
        sha256: hex::encode(hashing.hasher.finalize()),
    };
    Ok((summary, hashing.inner))
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// Decrypts and verifies records, yielding plaintext only after its record
/// authenticated. Errors on tampering, reordering and truncation.
pub struct DecryptReader<R: Read> {
    inner: BufReader<R>,
    cipher: Option<LessSafeKey>,
    header: Vec<u8>,
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    chunk_size: usize,
    counter: u32,
    record: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> DecryptReader<R> {
    /// Parse the header and pick the key: the first of `secrets` that
    /// authenticates the first record. `expected_id`, when given, must match
    /// the id in the (authenticated) header.
    pub fn open(input: R, secrets: &[&str], expected_id: Option<&str>) -> Result<Self> {
        let mut inner = BufReader::with_capacity(CHUNK_SIZE + TAG_LEN, input);
        let (header, raw) = Header::read(&mut inner)?;
        if let Some(expected) = expected_id {
            ensure!(
                header.snapshot_id == expected,
                "archive belongs to snapshot {}, not {expected}",
                header.snapshot_id
            );
        }
        let mut reader = Self {
            inner,
            cipher: None,
            header: raw,
            nonce_prefix: header.nonce_prefix,
            chunk_size: header.chunk_size,
            counter: 0,
            record: Vec::with_capacity(header.chunk_size + TAG_LEN),
            pos: 0,
            done: false,
        };
        let (ciphertext, last) = reader.read_record()?;
        for secret in secrets {
            let cipher = cipher_for(secret, &header.salt);
            let mut attempt = ciphertext.clone();
            if open_record(
                &cipher,
                &reader.header,
                &reader.nonce_prefix,
                0,
                last,
                &mut attempt,
            )
            .is_ok()
            {
                reader.cipher = Some(cipher);
                reader.record = attempt;
                reader.counter = 1;
                reader.done = last;
                return Ok(reader);
            }
        }
        bail!(
            "the snapshot archive can't be decrypted: it was made with a different \
             TOND_SECRET_KEY, or it was modified"
        )
    }

    /// The next raw record and whether it is the last one.
    fn read_record(&mut self) -> Result<(Vec<u8>, bool)> {
        let full = self.chunk_size + TAG_LEN;
        let mut record = Vec::with_capacity(full);
        (&mut self.inner)
            .take(full as u64)
            .read_to_end(&mut record)
            .context("failed to read the snapshot archive")?;
        ensure!(record.len() >= TAG_LEN, "the snapshot archive is truncated");
        let last = record.len() < full || self.inner.fill_buf()?.is_empty();
        Ok((record, last))
    }

    fn next_record(&mut self) -> Result<()> {
        let (mut record, last) = self.read_record()?;
        let cipher = self.cipher.as_ref().context("archive key not selected")?;
        open_record(
            cipher,
            &self.header,
            &self.nonce_prefix,
            self.counter,
            last,
            &mut record,
        )?;
        self.counter = self
            .counter
            .checked_add(1)
            .context("snapshot archive has too many records")?;
        self.record = record;
        self.pos = 0;
        self.done = last;
        Ok(())
    }
}

/// Authenticate and decrypt one record in place, leaving the plaintext.
fn open_record(
    cipher: &LessSafeKey,
    header: &[u8],
    prefix: &[u8; NONCE_PREFIX_LEN],
    counter: u32,
    last: bool,
    record: &mut Vec<u8>,
) -> Result<()> {
    let split = record.len() - TAG_LEN;
    let nonce = nonce_for(prefix, counter, last);
    cipher
        .open_in_place(nonce, Aad::from(header), record.as_mut_slice())
        .map_err(|_| {
            anyhow::anyhow!(
                "the snapshot archive failed authentication at record {counter}: \
                 it was modified or truncated"
            )
        })?;
    record.truncate(split);
    Ok(())
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.record.len() {
            if self.done {
                return Ok(0);
            }
            self.next_record().map_err(io::Error::other)?;
        }
        let n = out.len().min(self.record.len() - self.pos);
        out[..n].copy_from_slice(&self.record[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Read the whole archive, authenticating every record, without extracting.
/// Returns the snapshot id from the header.
pub fn verify_archive<R: Read>(
    input: R,
    secrets: &[&str],
    expected_id: Option<&str>,
) -> Result<String> {
    let mut reader = DecryptReader::open(input, secrets, expected_id)?;
    let id = String::from_utf8_lossy(&reader.header[8 + 1 + 4 + SALT_LEN + NONCE_PREFIX_LEN + 2..])
        .into_owned();
    io::copy(&mut reader, &mut io::sink()).context("snapshot archive verification failed")?;
    Ok(id)
}

/// Decrypt and extract an archive into `dest` (which must not exist yet or
/// be empty). Only regular files and directories with relative paths that
/// stay inside `dest` are accepted; anything else aborts the extraction.
/// Call `verify_archive` first: records are authenticated as they stream,
/// but truncation is only detectable at the end.
pub fn extract_archive<R: Read>(
    input: R,
    secrets: &[&str],
    expected_id: Option<&str>,
    dest: &Path,
) -> Result<()> {
    let reader = DecryptReader::open(input, secrets, expected_id)?;
    let mut archive = tar::Archive::new(GzDecoder::new(reader));
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_overwrite(true);
    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    for entry in archive.entries().context("failed to read the archive")? {
        let mut entry = entry.context("failed to read an archive entry")?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            bail!(
                "refusing to restore: the archive contains a {kind:?} entry \
                 (only files and directories are allowed)"
            );
        }
        let path = entry.path().context("archive entry has an invalid path")?;
        check_entry_path(&path)?;
        let shown = path.display().to_string();
        let unpacked = entry
            .unpack_in(dest)
            .with_context(|| format!("failed to restore {shown}"))?;
        ensure!(
            unpacked,
            "refusing to restore {shown}: it escapes the target"
        );
    }
    Ok(())
}

/// A tar entry path must be relative and must not climb out of the target.
fn check_entry_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty(),
        "archive entry has an empty path"
    );
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => bail!(
                "refusing to restore {}: the path leaves the snapshot directory",
                path.display()
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const SECRET: &str = "test-secret-key";

    fn sample_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        std::fs::create_dir_all(dir.path().join("volumes/web"))
            .unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::fs::write(dir.path().join("manifest.json"), b"{\"id\":\"s1\"}")
            .unwrap_or_else(|e| panic!("write: {e}"));
        // Several records' worth, not a multiple of the chunk size.
        // Incompressible, so the archive spans several records.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let big: Vec<u8> = (0..(CHUNK_SIZE * 3 + 123))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        std::fs::write(dir.path().join("volumes/web/data.bin"), &big)
            .unwrap_or_else(|e| panic!("write: {e}"));
        dir
    }

    fn archive_of(dir: &Path, id: &str) -> Vec<u8> {
        let (summary, bytes) = write_archive(dir, Vec::new(), SECRET, id)
            .unwrap_or_else(|e| panic!("write_archive: {e:#}"));
        assert_eq!(summary.bytes as usize, bytes.len());
        assert_eq!(summary.sha256, hex::encode(Sha256::digest(&bytes)));
        bytes
    }

    #[test]
    fn round_trips_files_and_directories() {
        let src = sample_dir();
        let archive = archive_of(src.path(), "s1");
        assert!(
            !archive
                .windows(b"manifest.json".len())
                .any(|w| w == b"manifest.json"),
            "file names must not be visible in the archive"
        );

        verify_archive(Cursor::new(&archive), &[SECRET], Some("s1"))
            .unwrap_or_else(|e| panic!("verify: {e:#}"));
        let out = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dest = out.path().join("restored");
        extract_archive(Cursor::new(&archive), &[SECRET], Some("s1"), &dest)
            .unwrap_or_else(|e| panic!("extract: {e:#}"));
        assert_eq!(
            std::fs::read(dest.join("volumes/web/data.bin")).ok(),
            std::fs::read(src.path().join("volumes/web/data.bin")).ok()
        );
        assert!(dest.join("manifest.json").is_file());
    }

    #[test]
    fn exact_multiple_of_the_chunk_size_round_trips() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let mut sink = Vec::new();
        let mut w =
            EncryptWriter::new(&mut sink, SECRET, "s").unwrap_or_else(|e| panic!("writer: {e:#}"));
        w.write_all(&vec![7u8; CHUNK_SIZE * 2])
            .unwrap_or_else(|e| panic!("write: {e}"));
        w.finish().unwrap_or_else(|e| panic!("finish: {e}"));
        let mut plain = Vec::new();
        DecryptReader::open(Cursor::new(&sink), &[SECRET], None)
            .unwrap_or_else(|e| panic!("open: {e:#}"))
            .read_to_end(&mut plain)
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(plain.len(), CHUNK_SIZE * 2);
        drop(dir);
    }

    #[test]
    fn rejects_tampering_truncation_wrong_key_and_wrong_snapshot() {
        let src = sample_dir();
        let archive = archive_of(src.path(), "s1");

        let mut flipped = archive.clone();
        let mid = flipped.len() / 2;
        flipped[mid] ^= 1;
        assert!(verify_archive(Cursor::new(&flipped), &[SECRET], None).is_err());

        // Drop the last record entirely (truncation at a record boundary)…
        let header_len = Header::read(&mut Cursor::new(&archive))
            .map(|(_, raw)| raw.len())
            .unwrap_or_else(|e| panic!("header: {e:#}"));
        let body = archive.len() - header_len;
        let records = body.div_ceil(CHUNK_SIZE + TAG_LEN);
        assert!(records >= 2, "sample must span several records");
        let cut = header_len + (records - 1) * (CHUNK_SIZE + TAG_LEN);
        assert!(verify_archive(Cursor::new(&archive[..cut]), &[SECRET], None).is_err());
        // …or cut inside a record.
        assert!(
            verify_archive(Cursor::new(&archive[..archive.len() - 5]), &[SECRET], None).is_err()
        );

        assert!(verify_archive(Cursor::new(&archive), &["other-key"], None).is_err());
        assert!(
            verify_archive(Cursor::new(&archive), &["other-key", SECRET], None).is_ok(),
            "the previous key is tried too (key rotation)"
        );
        assert!(verify_archive(Cursor::new(&archive), &[SECRET], Some("s2")).is_err());

        // A header edited to claim another snapshot no longer authenticates.
        let mut renamed = archive.clone();
        let id_pos = header_len - 2;
        renamed[id_pos] = b'X';
        assert!(verify_archive(Cursor::new(&renamed), &[SECRET], None).is_err());
    }

    /// Build an authentic archive around an arbitrary tar stream.
    fn seal_tar(tar_bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let enc = EncryptWriter::new(&mut out, SECRET, "evil")
            .unwrap_or_else(|e| panic!("writer: {e:#}"));
        let mut gz = GzEncoder::new(enc, Compression::fast());
        gz.write_all(tar_bytes)
            .unwrap_or_else(|e| panic!("gz: {e}"));
        gz.finish()
            .unwrap_or_else(|e| panic!("gz finish: {e}"))
            .finish()
            .unwrap_or_else(|e| panic!("seal: {e}"));
        out
    }

    fn raw_entry(name: &[u8], kind: tar::EntryType, link: Option<&str>) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        {
            let old = header.as_old_mut();
            old.name[..name.len()].copy_from_slice(name);
        }
        header.set_entry_type(kind);
        header.set_size(if kind.is_file() { 4 } else { 0 });
        header.set_mode(0o644);
        if let Some(link) = link {
            header
                .set_link_name(link)
                .unwrap_or_else(|e| panic!("link: {e}"));
        }
        header.set_cksum();
        let mut out = header.as_bytes().to_vec();
        if kind.is_file() {
            let mut data = b"evil".to_vec();
            data.resize(512, 0);
            out.extend_from_slice(&data);
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    #[test]
    fn refuses_links_devices_and_path_traversal() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "traversal",
                raw_entry(b"../escape.txt", tar::EntryType::Regular, None),
            ),
            (
                "absolute",
                raw_entry(b"/etc/tond-owned", tar::EntryType::Regular, None),
            ),
            (
                "symlink",
                raw_entry(
                    b"volumes/link",
                    tar::EntryType::Symlink,
                    Some("/etc/shadow"),
                ),
            ),
            (
                "hardlink",
                raw_entry(b"volumes/hard", tar::EntryType::Link, Some("/etc/passwd")),
            ),
            (
                "device",
                raw_entry(b"volumes/dev", tar::EntryType::Char, None),
            ),
            (
                "fifo",
                raw_entry(b"volumes/fifo", tar::EntryType::Fifo, None),
            ),
        ];
        for (what, tar_bytes) in cases {
            let archive = seal_tar(&tar_bytes);
            let out = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
            let dest = out.path().join("restore");
            let result = extract_archive(Cursor::new(&archive), &[SECRET], None, &dest);
            assert!(result.is_err(), "{what} must be refused");
            assert!(
                !out.path().join("escape.txt").exists(),
                "{what}: nothing may be written outside the target"
            );
        }
    }

    #[test]
    fn skips_symlinks_when_archiving() {
        let src = sample_dir();
        std::os::unix::fs::symlink("/etc/passwd", src.path().join("volumes/web/leak"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));
        let archive = archive_of(src.path(), "s1");
        let out = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dest = out.path().join("restored");
        extract_archive(Cursor::new(&archive), &[SECRET], None, &dest)
            .unwrap_or_else(|e| panic!("extract: {e:#}"));
        assert!(
            std::fs::symlink_metadata(dest.join("volumes/web/leak")).is_err(),
            "symlinks are not snapshot data and must not be archived"
        );
    }
}
