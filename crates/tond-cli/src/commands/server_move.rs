// commands/server_move.rs — `tond export` and `tond import`: move a whole
// server
//
// export: take a full snapshot (or use --snapshot) and write it as one
//   encrypted archive to a local file (`<file>` plus `<file>.manifest.json`),
//   and/or upload it to a storage provider (`--remote`).
// import: add an exported file (streamed to the server) or a snapshot from a
//   storage provider to the server's list, then restore it in full: apps,
//   databases, volumes, settings and still-valid certificates. The server
//   restarts and starts or redeploys every app and database.
//
// Archives are encrypted with the old server's TOND_SECRET_KEY: the new
// server needs the same key, and refuses with SNAPSHOT_KEY_MISMATCH otherwise.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use serde_json::{Value, json};
use tond_common::{code, exit};

use crate::client::TondClient;
use crate::error::CliError;
use crate::output::{Output, outln};

/// Format of the manifest written next to an exported file.
const EXPORT_MANIFEST_FORMAT: &str = "tond-export-manifest/1";

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// File to write the encrypted archive to (default: tond-export-<date>.tondsnap,
    /// unless only --remote is given)
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,

    /// Upload the archive (and its manifest) to a storage provider instead of,
    /// or as well as (with --output), writing a file
    #[arg(long)]
    pub remote: bool,

    /// Storage provider for --remote (default: the one in the backup settings)
    #[arg(long, requires = "remote")]
    pub provider: Option<String>,

    /// Export this existing snapshot instead of taking a new full snapshot
    #[arg(long)]
    pub snapshot: Option<String>,

    /// Name of the new snapshot
    #[arg(long, conflicts_with = "snapshot")]
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// An exported archive file, or the id of a snapshot in a storage provider
    pub source: String,

    /// Storage provider holding the snapshot (default: the one in the backup settings)
    #[arg(long)]
    pub provider: Option<String>,

    /// Only add the snapshot to the server's list; don't restore it
    #[arg(long)]
    pub no_restore: bool,

    /// Don't ask for confirmation (required when stdin is not a terminal)
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// What `export` writes next to the archive file.
pub fn export_manifest(manifest: &Value, id: &str, bytes: u64, sha256: &str) -> Value {
    let field = |k: &str| manifest.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "format": EXPORT_MANIFEST_FORMAT,
        "id": id,
        "name": field("name"),
        "created_at": field("created_at"),
        "tond_version": field("tond_version"),
        "scope": field("scope"),
        "apps": field("apps_included"),
        "databases": field("databases_included"),
        "archive_bytes": bytes,
        "sha256": sha256,
        "note": "Encrypted with the TOND_SECRET_KEY of the server that made it. Import with: tond import <file>",
    })
}

fn manifest_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".manifest.json");
    PathBuf::from(name)
}

pub async fn handle_export(client: &TondClient, out: &Output, args: ExportArgs) -> Result<()> {
    let output = match (&args.output, args.remote) {
        (Some(path), _) => Some(path.clone()),
        (None, true) => None,
        (None, false) => Some(PathBuf::from(format!(
            "tond-export-{}.tondsnap",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ))),
    };
    if let Some(path) = &output
        && path.exists()
    {
        return Err(CliError::usage(format!("{} already exists", path.display()))
            .with_hint("Choose another --output, or remove the file first.")
            .into());
    }

    let snapshot = match &args.snapshot {
        Some(id) => id.clone(),
        None => {
            outln!(out, "Taking a full snapshot…");
            let name = args.name.clone().unwrap_or_else(|| {
                format!("Export {}", chrono::Utc::now().format("%Y-%m-%d %H:%M"))
            });
            let snap = client.create_snapshot(Some(&name)).await?;
            outln!(out, "Snapshot {} ({}) taken.", snap.name, snap.id);
            snap.id
        }
    };

    let mut result = json!({ "ok": true, "action": "export", "snapshot_id": snapshot });

    if let Some(path) = &output {
        outln!(out, "Writing the encrypted archive to {}…", path.display());
        let (bytes, sha256) = client.download_snapshot_archive(&snapshot, path).await?;
        let manifest = client.snapshot_manifest(&snapshot).await?;
        let sidecar = manifest_path(path);
        tokio::fs::write(
            &sidecar,
            serde_json::to_vec_pretty(&export_manifest(&manifest, &snapshot, bytes, &sha256))?,
        )
        .await?;
        outln!(
            out,
            "Wrote {} ({:.1} MB, sha256 {sha256}) and {}.",
            path.display(),
            bytes as f64 / 1_048_576.0,
            sidecar.display()
        );
        result["file"] = json!(path.display().to_string());
        result["manifest_file"] = json!(sidecar.display().to_string());
        result["bytes"] = json!(bytes);
        result["sha256"] = json!(sha256);
    }

    if args.remote {
        outln!(out, "Uploading to remote storage…");
        client
            .upload_snapshot_remote(&snapshot, args.provider.as_deref())
            .await?;
        let snap = wait_for_upload(client, &snapshot).await?;
        outln!(
            out,
            "Uploaded as {} (import it with: tond import {snapshot}).",
            snap.remote_key.as_deref().unwrap_or("?")
        );
        result["remote_key"] = json!(snap.remote_key);
    }

    out.json(&result)?;
    outln!(
        out,
        "The archive is encrypted with this server's TOND_SECRET_KEY (/opt/tond/data/tond.env). \
         The new server needs the same key."
    );
    Ok(())
}

/// Poll the snapshot until its upload finishes (or fails).
async fn wait_for_upload(client: &TondClient, id: &str) -> Result<tond_common::SnapshotResponse> {
    for _ in 0..(6 * 3600 / 2) {
        let snap = client
            .list_snapshots(None)
            .await?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| CliError::coded(code::NOT_FOUND, exit::NOT_FOUND, format!("snapshot {id} disappeared")))?;
        match snap.remote_status.as_deref() {
            Some("uploaded") => return Ok(snap),
            Some("failed") => {
                return Err(CliError::coded(code::INTERNAL, exit::GENERIC, format!(
                    "The upload failed: {}",
                    snap.remote_error.as_deref().unwrap_or("unknown error")
                ))
                .into());
            }
            _ => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
    Err(CliError::coded(code::INTERNAL, exit::GENERIC, "The upload did not finish in time").into())
}

pub async fn handle_import(client: &TondClient, out: &Output, args: ImportArgs) -> Result<()> {
    let file = Path::new(&args.source);
    let is_file = file.is_file();
    if !args.no_restore {
        out.confirm(
            args.yes,
            &format!(
                "Restore {} onto this server? Every app, database, volume and setting here is \
                 replaced with the snapshot's, and tond-server restarts. A pre-restore snapshot \
                 is taken first.",
                args.source
            ),
            "restore a snapshot onto this server",
        )?;
    }

    let snap = if is_file {
        outln!(out, "Uploading {} to the server…", file.display());
        client.upload_snapshot_archive(file).await?
    } else {
        outln!(out, "Importing snapshot {} from remote storage…", args.source);
        client
            .import_remote_snapshot(&args.source, args.provider.as_deref())
            .await?
    };
    outln!(out, "Snapshot {} ({}, {}) is in the list.", snap.name, snap.id, snap.scope);

    if args.no_restore {
        out.done("import", "snapshot", &snap.id, serde_json::to_value(&snap)?)?;
        return Ok(());
    }
    if snap.scope != "full" {
        return Err(CliError::usage(format!(
            "Snapshot {} holds only {}; restore it with `tond snapshot rollback {} --app/--database <name>`",
            snap.id, snap.scope, snap.id
        ))
        .into());
    }
    outln!(out, "Restoring (the snapshot is downloaded and verified first)…");
    let data = client.restore_snapshot_full(&snap.id).await?;
    out.done("import", "snapshot", &snap.id, data.clone())?;
    if let Some(pre) = data.get("pre_restore_snapshot_id").and_then(Value::as_str) {
        outln!(out, "Pre-restore snapshot: {pre}");
    }
    outln!(
        out,
        "tond-server restarts when the data is in place, then starts or redeploys every app \
         and database. Follow along with `tond apps list` and `journalctl -u tond-server -f`."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_manifest_names_what_the_archive_holds() {
        let manifest = json!({
            "name": "Export", "created_at": "2026-10-01T00:00:00Z", "tond_version": "0.2.0",
            "scope": "full", "apps_included": ["web"], "databases_included": ["db"],
        });
        let m = export_manifest(&manifest, "s1", 42, "ab");
        assert_eq!(m["format"], EXPORT_MANIFEST_FORMAT);
        assert_eq!(m["apps"], json!(["web"]));
        assert_eq!(m["archive_bytes"], 42);
        assert_eq!(
            manifest_path(Path::new("/tmp/x.tondsnap")),
            PathBuf::from("/tmp/x.tondsnap.manifest.json")
        );
    }
}
