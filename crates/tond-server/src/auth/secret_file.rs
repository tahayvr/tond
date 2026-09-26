// auth/secret_file.rs — root-only files for one-time credentials
//
// tond-server hands two credentials to the operator through files instead of
// logs (journald is readable by the `adm` group): the root API key and the
// first-run setup token. Both live in the data directory, mode 0600.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// File holding the root API key, written once when the key is generated.
pub const ROOT_API_KEY_FILE: &str = "root-api-key";
/// File holding the first-run setup token until the admin account exists.
pub const SETUP_TOKEN_FILE: &str = "setup-token";

/// The data directory: `TOND_DATA_DIR`, else the directory of `TOND_DB_PATH`,
/// else `/opt/tond/data`.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TOND_DATA_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let db_path =
        std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());
    Path::new(&db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/opt/tond/data"))
}

/// Atomically write `contents` (plus a newline) to `path` with mode 0600.
///
/// The temporary file is created 0600 from the start, so the secret is never
/// readable by other users, even briefly.
pub fn write_secret_file(path: &Path, contents: &str) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let file_name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{file_name}.tmp"));
    let _ = std::fs::remove_file(&tmp);

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);

    // create_new + mode is subject to the umask; make sure it's exactly 0600.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path).with_context(|| format!("moving {} into place", path.display()))?;
    Ok(())
}

/// Read a secret file written by `write_secret_file` (trimmed). None if missing.
pub fn read_secret_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_secret_file_creates_owner_only_file_and_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join("nested").join("secret");

        write_secret_file(&path, "first").unwrap_or_else(|err| panic!("write failed: {err}"));
        write_secret_file(&path, "second").unwrap_or_else(|err| panic!("rewrite failed: {err}"));

        let meta = std::fs::metadata(&path).unwrap_or_else(|err| panic!("metadata: {err}"));
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(read_secret_file(&path).as_deref(), Some("second"));
        assert!(
            !dir.path().join("nested").join(".secret.tmp").exists(),
            "the temporary file must not be left behind"
        );
    }

    #[test]
    fn read_secret_file_is_none_for_missing_or_blank_files() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        assert_eq!(read_secret_file(&dir.path().join("missing")), None);
        let blank = dir.path().join("blank");
        std::fs::write(&blank, "\n").unwrap_or_else(|err| panic!("write: {err}"));
        assert_eq!(read_secret_file(&blank), None);
    }
}
