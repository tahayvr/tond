// commands/snapshots.rs — `tond snapshot` subcommand

use anyhow::Result;
use clap::Subcommand;
use serde_json::{Value, json};

use crate::client::TondClient;
use crate::error::CliError;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum SnapshotAction {
    /// Create a manual full snapshot
    Create {
        /// Optional human-readable label for this snapshot
        #[arg(long)]
        name: Option<String>,
    },

    /// List snapshots
    #[command(visible_alias = "ls")]
    List {
        /// Filter to snapshots for a specific app name
        #[arg(long)]
        app: Option<String>,
    },

    /// List the snapshots in a storage provider's bucket (also ones this
    /// server never took: restore them with `tond import <id>`)
    Remote {
        /// Storage provider to list (default: the one in the backup settings)
        #[arg(long)]
        provider: Option<String>,
    },

    /// Restore a full snapshot (asks first; the server restarts)
    Restore {
        /// Snapshot ID to restore from
        id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Roll back a single app or database to a snapshot state (no server
    /// restart; asks first)
    Rollback {
        /// Snapshot ID to roll back from
        id: String,

        /// Name of the app to roll back
        #[arg(long, conflicts_with = "database")]
        app: Option<String>,

        /// Name of the managed database to restore
        #[arg(long, conflicts_with = "app")]
        database: Option<String>,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Delete a snapshot (asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Snapshot ID to delete
        id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Backup configuration sub-commands
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current backup configuration
    #[command(visible_alias = "get")]
    Show,

    /// Update backup configuration
    #[command(visible_alias = "update")]
    Set {
        /// Cron expression for scheduled backups (e.g. "0 3 * * *")
        #[arg(long)]
        schedule: Option<String>,

        /// Number of full snapshots to retain
        #[arg(long)]
        retain: Option<u32>,

        /// Enable or disable pre-deploy snapshots (true/false)
        #[arg(long)]
        pre_deploy_enabled: Option<bool>,

        /// Number of pre-deploy snapshots to retain per app
        #[arg(long)]
        pre_deploy_retain: Option<u32>,

        /// Keep a local copy of snapshots (true/false)
        #[arg(long)]
        keep_local: Option<bool>,

        /// Storage provider ID to upload snapshots to (use "none" to clear)
        #[arg(long)]
        provider: Option<String>,
    },
}

pub async fn handle(client: &TondClient, out: &Output, action: SnapshotAction) -> Result<()> {
    match action {
        SnapshotAction::Create { name } => {
            outln!(out, "Creating snapshot…");
            let snapshot = client.create_snapshot(name.as_deref()).await?;
            out.json(&snapshot)?;
            outln!(out, "Snapshot created: {} ({})", snapshot.name, snapshot.id);
            if snapshot.remote_status.as_deref() == Some("uploading") {
                outln!(
                    out,
                    "An encrypted copy is uploading to remote storage; check with: tond snapshot list"
                );
            }
            outln!(
                out,
                "Remote copies are encrypted with this server's TOND_SECRET_KEY \
                 (/opt/tond/data/tond.env). Keep a copy of it somewhere safe: restoring \
                 on a new server needs the same key."
            );
        }

        SnapshotAction::List { app } => {
            let list = client.list_snapshots(app.as_deref()).await?;
            out.json(&list)?;
            if list.is_empty() {
                outln!(out, "No snapshots found.");
            } else {
                outln!(
                    out,
                    "{:<28} {:<10} {:<12} {:<8} CREATED AT",
                    "ID",
                    "TRIGGER",
                    "SCOPE",
                    "STATUS"
                );
                outln!(out, "{}", "-".repeat(90));
                for s in &list {
                    outln!(
                        out,
                        "{:<28} {:<10} {:<12} {:<8} {}",
                        s.id,
                        s.trigger,
                        s.scope,
                        s.status,
                        s.created_at,
                    );
                }
            }
        }

        SnapshotAction::Remote { provider } => {
            let list = client.list_remote_snapshots(provider.as_deref()).await?;
            out.json(&list)?;
            if list.is_empty() {
                outln!(out, "No snapshots in this bucket.");
            } else {
                outln!(
                    out,
                    "{:<38} {:<20} {:<22} {:>10}  NAME",
                    "ID",
                    "SCOPE",
                    "CREATED AT",
                    "SIZE"
                );
                for s in &list {
                    let name = if s.readable {
                        format!(
                            "{}{}",
                            s.name.as_deref().unwrap_or(""),
                            if s.imported { " (in list)" } else { "" }
                        )
                    } else {
                        "(made with a different TOND_SECRET_KEY)".to_string()
                    };
                    outln!(
                        out,
                        "{:<38} {:<20} {:<22} {:>8.1}MB  {}",
                        s.id,
                        s.scope.as_deref().unwrap_or("?"),
                        s.created_at,
                        s.archive_bytes as f64 / 1_048_576.0,
                        name
                    );
                }
            }
        }

        SnapshotAction::Restore { id, yes } => {
            out.confirm(
                yes,
                &format!(
                    "Restore snapshot {id}? Every app, database and setting is replaced with the \
                     snapshot's state and the Tond server restarts. A pre-restore snapshot is \
                     taken first."
                ),
                &format!("restore snapshot {id}"),
            )?;
            outln!(out, "Initiating restore…");
            let data = client.restore_snapshot_full(&id).await?;
            out.done("restore", "snapshot", &id, data.clone())?;
            if let Some(pre) = data.get("pre_restore_snapshot_id").and_then(Value::as_str) {
                outln!(out, "Pre-restore snapshot: {pre}");
            }
            outln!(out, "The server is restarting. Reconnect in a few seconds.");
        }

        SnapshotAction::Rollback {
            id,
            app,
            database,
            yes,
        } => match (app, database) {
            (Some(app_name), None) => {
                out.confirm(
                    yes,
                    &format!(
                        "Roll app {app_name} back to snapshot {id}? Its volumes and settings are overwritten."
                    ),
                    &format!("roll back app {app_name}"),
                )?;
                outln!(out, "Rolling back app '{}' to snapshot {}…", app_name, id);
                let data = client.restore_app_from_snapshot(&id, &app_name).await?;
                out.json(&json!({
                    "ok": true, "action": "rollback", "resource": "app", "id": app_name,
                    "snapshot_id": id, "data": data,
                }))?;
                outln!(out, "App '{}' rolled back successfully.", app_name);
            }
            (None, Some(db_name)) => {
                out.confirm(
                    yes,
                    &format!(
                        "Restore database {db_name} from snapshot {id}? Its current data is overwritten."
                    ),
                    &format!("restore database {db_name}"),
                )?;
                outln!(
                    out,
                    "Restoring database '{}' from snapshot {}…",
                    db_name,
                    id
                );
                let data = client.rollback_database(&id, &db_name).await?;
                out.json(&json!({
                    "ok": true, "action": "rollback", "resource": "database", "id": db_name,
                    "snapshot_id": id, "data": data,
                }))?;
                outln!(out, "Database '{}' restored successfully.", db_name);
            }
            _ => {
                return Err(
                    CliError::usage("Specify either --app <name> or --database <name>").into(),
                );
            }
        },

        SnapshotAction::Delete { id, yes } => {
            out.confirm(
                yes,
                &format!("Delete snapshot {id}? This cannot be undone."),
                &format!("delete snapshot {id}"),
            )?;
            let data = client.delete_snapshot(&id).await?;
            out.done("delete", "snapshot", &id, data)?;
            outln!(out, "Snapshot {} deleted.", id);
        }

        SnapshotAction::Config { action } => match action {
            ConfigAction::Show => {
                let cfg = client.get_backup_config().await?;
                out.json(&cfg)?;
                outln!(
                    out,
                    "Scheduled backups:   {}",
                    if cfg.enabled { "enabled" } else { "disabled" }
                );
                outln!(out, "Schedule (cron):     {}", cfg.full_schedule_cron);
                outln!(out, "Retain (full):       {}", cfg.full_retention_count);
                outln!(
                    out,
                    "Pre-deploy:          {}",
                    if cfg.pre_deploy_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                outln!(
                    out,
                    "Pre-deploy retain:   {}",
                    cfg.pre_deploy_retention_per_app
                );
                outln!(
                    out,
                    "Keep local:          {}",
                    if cfg.keep_local { "yes" } else { "no" }
                );
                match &cfg.storage_provider_id {
                    Some(pid) => outln!(out, "Storage provider:    {}", pid),
                    None => outln!(out, "Storage provider:    (none — local only)"),
                }
            }

            ConfigAction::Set {
                schedule,
                retain,
                pre_deploy_enabled,
                pre_deploy_retain,
                keep_local,
                provider,
            } => {
                let mut body = json!({});
                if let Some(s) = schedule {
                    body["full_schedule_cron"] = json!(s);
                }
                if let Some(r) = retain {
                    body["full_retention_count"] = json!(r);
                }
                if let Some(p) = pre_deploy_enabled {
                    body["pre_deploy_enabled"] = json!(p);
                }
                if let Some(r) = pre_deploy_retain {
                    body["pre_deploy_retention_per_app"] = json!(r);
                }
                if let Some(k) = keep_local {
                    body["keep_local"] = json!(k);
                }
                if let Some(p) = provider {
                    if p == "none" {
                        body["storage_provider_id"] = json!(null);
                    } else {
                        body["storage_provider_id"] = json!(p);
                    }
                }

                if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err(CliError::usage(
                        "No fields specified. Use --help to see available options.",
                    )
                    .into());
                }

                let cfg = client.update_backup_config(body).await?;
                out.json(&cfg)?;
                outln!(out, "Backup configuration updated.");
                outln!(
                    out,
                    "Scheduled backups:   {}",
                    if cfg.enabled { "enabled" } else { "disabled" }
                );
                outln!(out, "Schedule (cron):     {}", cfg.full_schedule_cron);
                outln!(out, "Retain (full):       {}", cfg.full_retention_count);
            }
        },
    }

    Ok(())
}
