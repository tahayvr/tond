// commands/snapshots.rs — `tond snapshot` subcommand

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum SnapshotAction {
    /// Create a manual full snapshot (streams progress)
    Create {
        /// Optional human-readable label for this snapshot
        #[arg(long)]
        name: Option<String>,
    },

    /// List snapshots
    List {
        /// Filter to snapshots for a specific app name
        #[arg(long)]
        app: Option<String>,
    },

    /// Restore a full snapshot (prompts for confirmation; server will restart)
    Restore {
        /// Snapshot ID to restore from
        id: String,
    },

    /// Roll back a single app or database to a snapshot state (no server restart)
    Rollback {
        /// Snapshot ID to roll back from
        id: String,

        /// Name of the app to roll back
        #[arg(long, conflicts_with = "database")]
        app: Option<String>,

        /// Name of the managed database to restore
        #[arg(long, conflicts_with = "app")]
        database: Option<String>,
    },

    /// Delete a snapshot
    Delete {
        /// Snapshot ID to delete
        id: String,
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
    Show,

    /// Update backup configuration
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

pub async fn handle(client: &TondClient, action: SnapshotAction) -> Result<()> {
    match action {
        SnapshotAction::Create { name } => {
            println!("Creating snapshot…");
            let snap = client.create_snapshot(name.as_deref()).await?;
            println!("Snapshot created: {} ({})", snap.name, snap.id);
            println!("Status: {}", snap.status);
        }

        SnapshotAction::List { app } => {
            let list = client.list_snapshots(app.as_deref()).await?;
            if list.is_empty() {
                println!("No snapshots found.");
            } else {
                println!(
                    "{:<28} {:<10} {:<12} {:<8} CREATED AT",
                    "ID", "TRIGGER", "SCOPE", "STATUS"
                );
                println!("{}", "-".repeat(90));
                for s in &list {
                    println!(
                        "{:<28} {:<10} {:<12} {:<8} {}",
                        s.id, s.trigger, s.scope, s.status, s.created_at,
                    );
                }
            }
        }

        SnapshotAction::Restore { id } => {
            println!("WARNING: Full restore will restart the Tond server.");
            println!("A pre-restore snapshot will be created automatically first.");
            println!();
            print!("Type 'yes' to confirm: ");
            // Flush stdout so prompt appears before we read
            use std::io::Write;
            std::io::stdout().flush()?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if input.trim() != "yes" {
                println!("Aborted.");
                return Ok(());
            }

            println!("Initiating restore…");
            let msg = client.restore_snapshot_full(&id).await?;
            println!("{}", msg);
            println!("The server is restarting. Reconnect in a few seconds.");
        }

        SnapshotAction::Rollback { id, app, database } => match (app, database) {
            (Some(app_name), None) => {
                println!("Rolling back app '{}' to snapshot {}…", app_name, id);
                client.restore_app_from_snapshot(&id, &app_name).await?;
                println!("App '{}' rolled back successfully.", app_name);
            }
            (None, Some(db_name)) => {
                println!("Restoring database '{}' from snapshot {}…", db_name, id);
                client.rollback_database(&id, &db_name).await?;
                println!("Database '{}' restored successfully.", db_name);
            }
            _ => {
                anyhow::bail!("Specify either --app <name> or --database <name>");
            }
        },

        SnapshotAction::Delete { id } => {
            client.delete_snapshot(&id).await?;
            println!("Snapshot {} deleted.", id);
        }

        SnapshotAction::Config { action } => match action {
            ConfigAction::Show => {
                let cfg = client.get_backup_config().await?;
                println!(
                    "Scheduled backups:   {}",
                    if cfg.enabled { "enabled" } else { "disabled" }
                );
                println!("Schedule (cron):     {}", cfg.full_schedule_cron);
                println!("Retain (full):       {}", cfg.full_retention_count);
                println!(
                    "Pre-deploy:          {}",
                    if cfg.pre_deploy_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                println!("Pre-deploy retain:   {}", cfg.pre_deploy_retention_per_app);
                println!(
                    "Keep local:          {}",
                    if cfg.keep_local { "yes" } else { "no" }
                );
                match &cfg.storage_provider_id {
                    Some(pid) => println!("Storage provider:    {}", pid),
                    None => println!("Storage provider:    (none — local only)"),
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
                    anyhow::bail!("No fields specified. Use --help to see available options.");
                }

                let cfg = client.update_backup_config(body).await?;
                println!("Backup configuration updated.");
                println!(
                    "Scheduled backups:   {}",
                    if cfg.enabled { "enabled" } else { "disabled" }
                );
                println!("Schedule (cron):     {}", cfg.full_schedule_cron);
                println!("Retain (full):       {}", cfg.full_retention_count);
            }
        },
    }

    Ok(())
}
