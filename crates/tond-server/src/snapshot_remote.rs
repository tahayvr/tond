// snapshot_remote.rs — snapshots another server can find: manifests,
// bucket listings, imports and exports
//
// Every archive uploaded to a storage provider gets a small manifest next to
// it (`snapshots/<id>.manifest.json`), so a server that has never seen the
// snapshot — a fresh install after a server move — can list the bucket and
// import it without the old server's database:
//
//   { "format": "tond-snapshot-manifest/1", "id", "created_at",
//     "tond_version", "archive_key", "archive_bytes", "sha256",
//     "sealed": "v1.…" }
//
// `sealed` holds the names (snapshot name, scope, apps and databases)
// encrypted with TOND_SECRET_KEY (`Purpose::SnapshotManifest`), so the
// storage provider learns nothing it can't already see, and opening it is
// the key check: a manifest this server can't open was made with another
// TOND_SECRET_KEY, which is reported (`SNAPSHOT_KEY_MISMATCH`) before
// anything is downloaded. The sealed part repeats the id and the archive's
// SHA-256, so the plain fields can't be swapped for another snapshot's.
//
// Importing creates a snapshot row (remote copy only); the existing restore
// paths then download, check and verify the archive. Exports are the same
// archive streamed to the client (`tond export`), and `tond import <file>`
// sends it back: the archive is verified, extracted into a local snapshot
// and listed like any other.

use std::path::Path;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tond_common::{RemoteSnapshotResponse, Snapshot, StorageProvider, TondError};

use crate::error::AppError;
use crate::handlers::storage_providers::decrypt_provider_secret;
use crate::secret_key::{self, Key, Purpose};
use crate::snapshot_archive::{ArchiveSummary, WrongKey};
use crate::state::AppState;
use crate::{backup, storage};

pub const MANIFEST_FORMAT: &str = "tond-snapshot-manifest/1";
const MANIFEST_SUFFIX: &str = ".manifest.json";
const SNAPSHOTS_PREFIX: &str = "snapshots/";
/// A manifest is a few hundred bytes; refuse anything absurd.
const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Object key of a snapshot's archive.
pub fn archive_key(id: &str) -> String {
    format!("{SNAPSHOTS_PREFIX}{id}.tondsnap")
}

/// Object key of a snapshot's manifest.
pub fn manifest_key(id: &str) -> String {
    format!("{SNAPSHOTS_PREFIX}{id}{MANIFEST_SUFFIX}")
}

/// Snapshot ids name directories and object keys: letters, digits and
/// dashes only (Tond makes UUIDs).
pub fn valid_snapshot_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The manifest stored next to an archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteManifest {
    pub format: String,
    pub id: String,
    pub created_at: String,
    pub tond_version: String,
    pub archive_key: String,
    pub archive_bytes: u64,
    pub sha256: String,
    /// `ManifestDetails` as JSON, encrypted (`Purpose::SnapshotManifest`)
    pub sealed: String,
}

/// The sealed part of a manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestDetails {
    pub id: String,
    pub sha256: String,
    pub name: String,
    pub trigger: String,
    pub scope: String,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub databases: Vec<String>,
    #[serde(default)]
    pub total_bytes: Option<i64>,
}

fn strings(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The remote manifest of an archive, from the snapshot's own manifest.
pub fn build_manifest(
    local: &serde_json::Value,
    id: &str,
    key: &str,
    summary: &ArchiveSummary,
) -> Result<RemoteManifest> {
    let details = ManifestDetails {
        id: id.to_string(),
        sha256: summary.sha256.clone(),
        name: text(local, "name"),
        trigger: text(local, "trigger"),
        scope: text(local, "scope"),
        apps: strings(local, "apps_included"),
        databases: strings(local, "databases_included"),
        total_bytes: local.get("total_bytes").and_then(|v| v.as_i64()),
    };
    let sealed = secret_key::encrypt(
        Purpose::SnapshotManifest,
        &serde_json::to_string(&details)?,
    )?;
    Ok(RemoteManifest {
        format: MANIFEST_FORMAT.to_string(),
        id: id.to_string(),
        created_at: text(local, "created_at"),
        tond_version: text(local, "tond_version"),
        archive_key: key.to_string(),
        archive_bytes: summary.bytes,
        sha256: summary.sha256.clone(),
        sealed,
    })
}

/// `build_manifest` from a local snapshot directory.
pub async fn remote_manifest(
    snapshot: &Path,
    id: &str,
    key: &str,
    summary: &ArchiveSummary,
) -> Result<RemoteManifest> {
    let local = backup::read_manifest(snapshot).await?;
    build_manifest(&local, id, key, summary)
}

/// Open the sealed part with TOND_SECRET_KEY (or the previous key). None
/// when neither opens it — another server's key — or it doesn't match the
/// plain fields.
pub fn open_details(manifest: &RemoteManifest) -> Option<ManifestDetails> {
    let mut keys = vec![Key::current()];
    keys.extend(secret_key::previous_secret().map(|s| Key::derive(&s)));
    let json = keys.iter().find_map(|k| {
        secret_key::decrypt_with(k, Purpose::SnapshotManifest, &manifest.sealed).ok()
    })?;
    let details: ManifestDetails = serde_json::from_str(&json).ok()?;
    (details.id == manifest.id && details.sha256 == manifest.sha256).then_some(details)
}

fn key_mismatch() -> AppError {
    AppError(TondError::SnapshotKeyMismatch(
        "This snapshot was encrypted with a different TOND_SECRET_KEY, so this server can't \
         read it."
            .to_string(),
    ))
}

/// The storage provider `id` (default: the one in the backup settings),
/// with its secret decrypted.
pub async fn provider(state: &AppState, id: Option<String>) -> Result<StorageProvider, AppError> {
    let id = match id.filter(|s| !s.trim().is_empty()) {
        Some(id) => id,
        None => backup::get_backup_config(state)
            .await
            .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?
            .storage_provider_id
            .ok_or_else(|| {
                AppError(
                    TondError::invalid_field(
                        "provider_id",
                        "No storage provider given and none is set in the backup settings.",
                    )
                    .with_hint("Add the storage provider (same bucket and keys) first, then pick it."),
                )
            })?,
    };
    let row = sqlx::query_as::<_, StorageProvider>("SELECT * FROM storage_providers WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("storage provider {id}"))))?;
    decrypt_provider_secret(row).map_err(|e| {
        AppError(TondError::Internal(format!(
            "can't read the storage provider's secret: {e:#}"
        )))
    })
}

async fn fetch_manifest(provider: &StorageProvider, id: &str) -> Result<RemoteManifest> {
    let bytes = storage::get_object(provider, &manifest_key(id), MAX_MANIFEST_BYTES).await?;
    let manifest: RemoteManifest =
        serde_json::from_slice(&bytes).context("the snapshot manifest is unreadable")?;
    anyhow::ensure!(
        manifest.format == MANIFEST_FORMAT && manifest.id == id,
        "the snapshot manifest of {id} is not one Tond can read"
    );
    Ok(manifest)
}

/// What a manifest says, for the API.
pub fn describe(
    manifest: &RemoteManifest,
    provider_id: &str,
    imported: bool,
) -> RemoteSnapshotResponse {
    let details = open_details(manifest);
    RemoteSnapshotResponse {
        id: manifest.id.clone(),
        provider_id: provider_id.to_string(),
        created_at: manifest.created_at.clone(),
        tond_version: manifest.tond_version.clone(),
        archive_bytes: manifest.archive_bytes as i64,
        readable: details.is_some(),
        name: details.as_ref().map(|d| d.name.clone()),
        trigger: details.as_ref().map(|d| d.trigger.clone()),
        scope: details.as_ref().map(|d| d.scope.clone()),
        apps: details.as_ref().map(|d| d.apps.clone()).unwrap_or_default(),
        databases: details.map(|d| d.databases).unwrap_or_default(),
        imported,
    }
}

/// Every snapshot with a manifest in the provider's bucket, newest first.
pub async fn list_remote(
    state: &AppState,
    provider_id: Option<String>,
) -> Result<Vec<RemoteSnapshotResponse>, AppError> {
    let provider = provider(state, provider_id).await?;
    let objects = storage::list(&provider, SNAPSHOTS_PREFIX)
        .await
        .map_err(|e| AppError(TondError::Unavailable(format!("Listing the bucket failed: {e:#}"))))?;
    let ids: Vec<String> = objects
        .iter()
        .filter_map(|o| {
            o.key
                .strip_prefix(SNAPSHOTS_PREFIX)?
                .strip_suffix(MANIFEST_SUFFIX)
                .filter(|id| valid_snapshot_id(id))
                .map(str::to_string)
        })
        .collect();
    let manifests: Vec<RemoteManifest> = futures::stream::iter(ids)
        .map(|id| {
            let provider = &provider;
            async move {
                match fetch_manifest(provider, &id).await {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::warn!("Skipping remote snapshot {id}: {e:#}");
                        None
                    }
                }
            }
        })
        .buffer_unordered(8)
        .filter_map(|m| async move { m })
        .collect()
        .await;
    let known: Vec<String> = sqlx::query_scalar("SELECT id FROM snapshots")
        .fetch_all(&state.db)
        .await?;
    let mut out: Vec<RemoteSnapshotResponse> = manifests
        .iter()
        .map(|m| describe(m, &provider.id, known.contains(&m.id)))
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

/// Add a snapshot from the provider's bucket to this server's list (remote
/// copy only; a restore downloads and verifies it). Returns the existing
/// row when it is already listed. Refused with `SNAPSHOT_KEY_MISMATCH` when
/// this server's key can't open it.
pub async fn import_remote(
    state: &AppState,
    provider_id: Option<String>,
    id: &str,
) -> Result<Snapshot, AppError> {
    if !valid_snapshot_id(id) {
        return Err(AppError(TondError::invalid_field("id", "Not a snapshot id.")));
    }
    if let Ok(existing) = backup::fetch_snapshot(state, id).await {
        return Ok(existing);
    }
    let provider = provider(state, provider_id).await?;
    let manifest = fetch_manifest(&provider, id).await.map_err(|e| {
        AppError(TondError::NotFound(format!(
            "snapshot {id} in this bucket ({e:#})"
        )))
    })?;
    let details = open_details(&manifest).ok_or_else(key_mismatch)?;
    if manifest.archive_key != archive_key(id) {
        return Err(AppError(TondError::InvalidInput(format!(
            "The manifest of {id} points at an unexpected object."
        ))));
    }
    sqlx::query(
        "INSERT INTO snapshots (id, name, status, trigger, scope, created_at, completed_at,
                                total_bytes, remote_key, storage_provider_id, checksum_sha256,
                                archive_bytes, remote_status, notes)
         VALUES (?, ?, 'completed', ?, ?, ?, ?, ?, ?, ?, ?, ?, 'uploaded', ?)",
    )
    .bind(id)
    .bind(&details.name)
    .bind(&details.trigger)
    .bind(&details.scope)
    .bind(sqlite_time(&manifest.created_at))
    .bind(sqlite_time(&manifest.created_at))
    .bind(details.total_bytes)
    .bind(&manifest.archive_key)
    .bind(&provider.id)
    .bind(&manifest.sha256)
    .bind(manifest.archive_bytes as i64)
    .bind(format!("Imported from storage provider {}", provider.name))
    .execute(&state.db)
    .await?;
    Ok(backup::fetch_snapshot(state, id)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?)
}

/// `created_at` as SQLite's `datetime('now')` writes it (how the rest of the
/// table sorts), from an RFC 3339 time.
fn sqlite_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string())
}

/// Import an exported archive (a file on this server, e.g. an upload just
/// received): verified, extracted into a local snapshot, listed. Returns
/// the snapshot (the existing one when this id is already listed with a
/// local copy).
pub async fn import_archive_file(state: &AppState, archive: &Path) -> Result<Snapshot, AppError> {
    let path = archive.to_path_buf();
    let verified = tokio::task::spawn_blocking(move || backup::verify_archive_file(&path, None))
        .await
        .map_err(|e| AppError(TondError::Internal(format!("import task failed: {e}"))))?;
    let id = match verified {
        Ok(id) => id,
        Err(e) if e.downcast_ref::<WrongKey>().is_some() => return Err(key_mismatch()),
        Err(e) => {
            return Err(AppError(TondError::InvalidInput(format!(
                "Not a Tond snapshot archive this server can import: {e:#}"
            ))));
        }
    };
    if !valid_snapshot_id(&id) {
        return Err(AppError(TondError::InvalidInput(
            "The archive names an invalid snapshot id.".to_string(),
        )));
    }
    if let Ok(existing) = backup::fetch_snapshot(state, &id).await
        && existing
            .local_path
            .as_deref()
            .is_some_and(|p| Path::new(p).is_dir())
    {
        return Ok(existing);
    }

    let dest = backup::snapshot_dir(&id);
    let (src, out, sid) = (archive.to_path_buf(), dest.clone(), id.clone());
    tokio::task::spawn_blocking(move || backup::extract_archive_file(&src, &out, &sid))
        .await
        .map_err(|e| AppError(TondError::Internal(format!("import task failed: {e}"))))?
        .map_err(|e| AppError(TondError::Internal(format!("Extracting the archive failed: {e:#}"))))?;
    let local = backup::read_manifest(&dest)
        .await
        .map_err(|e| AppError(TondError::InvalidInput(format!("{e:#}"))))?;
    let created = text(&local, "created_at");
    sqlx::query(
        "INSERT INTO snapshots (id, name, status, trigger, scope, created_at, completed_at,
                                delta_bytes, total_bytes, local_path, notes)
         VALUES (?, ?, 'completed', ?, ?, ?, datetime('now'), ?, ?, ?, 'Imported from an archive')
         ON CONFLICT(id) DO UPDATE SET local_path = excluded.local_path, status = 'completed'",
    )
    .bind(&id)
    .bind(text(&local, "name"))
    .bind(text(&local, "trigger"))
    .bind(text(&local, "scope"))
    .bind(sqlite_time(&created))
    .bind(local.get("total_bytes").and_then(|v| v.as_i64()))
    .bind(local.get("total_bytes").and_then(|v| v.as_i64()))
    .bind(dest.to_string_lossy().as_ref())
    .execute(&state.db)
    .await?;
    Ok(backup::fetch_snapshot(state, &id)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ArchiveSummary {
        ArchiveSummary {
            bytes: 1234,
            sha256: "ab".repeat(32),
        }
    }

    fn local() -> serde_json::Value {
        serde_json::json!({
            "id": "11111111-2222-3333-4444-555555555555",
            "name": "Nightly",
            "trigger": "scheduled",
            "scope": "full",
            "created_at": "2026-10-01T03:00:00+00:00",
            "tond_version": "0.2.0",
            "apps_included": ["web", "blog"],
            "databases_included": ["shop-db"],
            "total_bytes": 99,
        })
    }

    #[test]
    fn manifests_seal_the_names_and_open_only_with_the_key() {
        let id = "11111111-2222-3333-4444-555555555555";
        let manifest = build_manifest(&local(), id, &archive_key(id), &summary())
            .unwrap_or_else(|e| panic!("manifest: {e:#}"));
        let json = serde_json::to_string(&manifest).unwrap_or_default();
        assert!(!json.contains("shop-db") && !json.contains("Nightly"), "names are sealed: {json}");
        assert_eq!(manifest.archive_bytes, 1234);

        let details = open_details(&manifest).unwrap_or_else(|| panic!("opens with the key"));
        assert_eq!(details.apps, vec!["web", "blog"]);
        assert_eq!(details.databases, vec!["shop-db"]);
        assert_eq!(details.scope, "full");

        let described = describe(&manifest, "sp1", false);
        assert!(described.readable);
        assert_eq!(described.name.as_deref(), Some("Nightly"));

        // Another server's key: unreadable, and nothing leaks.
        let mut foreign = manifest.clone();
        foreign.sealed = secret_key::encrypt_with(
            &Key::derive("another-server-key"),
            Purpose::SnapshotManifest,
            "{}",
        )
        .unwrap_or_default();
        let described = describe(&foreign, "sp1", false);
        assert!(!described.readable);
        assert!(described.name.is_none() && described.apps.is_empty());

        // The plain fields can't be pointed at another archive.
        let mut swapped = manifest.clone();
        swapped.sha256 = "cd".repeat(32);
        assert!(open_details(&swapped).is_none());
    }

    #[test]
    fn snapshot_ids_are_plain_names() {
        assert!(valid_snapshot_id("11111111-2222-3333-4444-555555555555"));
        for bad in ["", "../etc", "a/b", "a.b", &"x".repeat(65)] {
            assert!(!valid_snapshot_id(bad), "{bad}");
        }
        assert_eq!(sqlite_time("2026-10-01T03:00:00+02:00"), "2026-10-01 01:00:00");
    }
}
