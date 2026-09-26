// commands/db.rs — `tond db` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;
use crate::output::{Output, outln};

/// `--db-type`'s possible values: every engine the server supports.
fn db_type_parser() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(tond_common::ALL_ENGINES.iter().map(|e| e.db_type))
}

#[derive(Subcommand)]
pub enum DbAction {
    /// List all managed databases
    #[command(visible_alias = "ls")]
    List,

    /// Create a new managed database
    Create {
        /// Database name (alphanumeric + hyphens)
        name: String,

        /// Database engine
        #[arg(short = 't', long, value_parser = db_type_parser())]
        db_type: String,

        /// Project ID to attach to (optional)
        #[arg(long)]
        project: Option<String>,

        /// Memory limit in megabytes (default: none set; every container is
        /// capped at 75% of the server's RAM)
        #[arg(long)]
        memory_mb: Option<i64>,
    },

    /// Show details for a managed database (passwords masked)
    #[command(visible_aliases = ["show", "get"])]
    Info {
        /// Database name or ID
        name: String,
    },

    /// Reveal a database's passwords and connection strings (audit-logged)
    Credentials {
        /// Database name or ID
        name: String,
    },

    /// Delete a managed database (stops container and removes volume; asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Database name or ID
        name: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Stop a running managed database
    Stop {
        /// Database name or ID
        name: String,
    },

    /// Start a stopped managed database
    Start {
        /// Database name or ID
        name: String,
    },
}

pub async fn handle(client: &TondClient, out: &Output, action: DbAction) -> Result<()> {
    match action {
        DbAction::List => {
            let dbs = client.list_databases().await?;
            out.json(&dbs)?;
            if dbs.is_empty() {
                outln!(
                    out,
                    "No managed databases yet. Create one with: tond db create <name> --type <postgres|mysql|redis>"
                );
                return Ok(());
            }
            outln!(
                out,
                "{:<20} {:<10} {:<8} {:<10} {:<26}",
                "NAME",
                "TYPE",
                "STATUS",
                "PORT",
                "CREATED"
            );
            outln!(out, "{}", "-".repeat(74));
            for db in &dbs {
                outln!(
                    out,
                    "{:<20} {:<10} {:<8} {:<10} {:<26}",
                    db.name,
                    db.db_type,
                    db.status,
                    db.host_port
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    db.created_at,
                );
            }
        }

        DbAction::Create {
            name,
            db_type,
            project,
            memory_mb,
        } => {
            let memory_limit = memory_mb.map(|m| m * 1_048_576);
            let created = client
                .create_database(&name, &db_type, project.as_deref(), memory_limit)
                .await?;
            out.json(&created)?;
            let db = &created.detail;
            outln!(
                out,
                "Database created: {} (ID: {})",
                db.database.name,
                db.database.id
            );
            outln!(out, "Type:   {}", db.database.db_type);
            outln!(out, "Status: {}", db.database.status);
            if let Some(port) = db.database.host_port {
                outln!(out, "Port:   {}", port);
            }
            if let Some(ref cs) = created
                .credentials
                .as_ref()
                .and_then(|c| c.connection_string.as_ref())
            {
                outln!(out, "Conn:   {}", cs);
                outln!(
                    out,
                    "The password is shown once. Later: tond db credentials {}",
                    db.database.name
                );
            }
        }

        DbAction::Info { name } => {
            let db = client.get_database(&name).await?;
            out.json(&db)?;
            outln!(out, "Name:    {}", db.database.name);
            outln!(out, "ID:      {}", db.database.id);
            outln!(out, "Type:    {}", db.database.db_type);
            outln!(out, "Status:  {}", db.database.status);
            outln!(
                out,
                "Project: {}",
                db.database.project_id.as_deref().unwrap_or("-")
            );
            match db.database.host_port {
                Some(p) => outln!(out, "Port:    {}", p),
                None => outln!(out, "Port:    -"),
            }
            match db.database.memory_limit {
                Some(bytes) => outln!(out, "Memory:  {} MiB", bytes / 1_048_576),
                None => outln!(out, "Memory:  default"),
            }
            if let Some(ref cs) = db.connection_string {
                outln!(out, "Conn:    {}", cs);
            }
            if let Some(ref ics) = db.internal_connection_string {
                outln!(out, "Internal:{}", ics);
            }
            outln!(out, "Created: {}", db.database.created_at);
        }

        DbAction::Credentials { name } => {
            let creds = client.get_database_credentials(&name).await?;
            out.json(&creds)?;
            outln!(out, "User:     {}", creds.db_user.as_deref().unwrap_or("-"));
            outln!(
                out,
                "Password: {}",
                creds.db_password.as_deref().unwrap_or("-")
            );
            if let Some(ref root) = creds.root_password {
                outln!(out, "Root:     {root}");
            }
            if let Some(ref cs) = creds.connection_string {
                outln!(out, "Conn:     {cs}");
            }
            if let Some(ref cs) = creds.internal_connection_string {
                outln!(out, "Internal: {cs}");
            }
            if let Some(ref cs) = creds.public_connection_string {
                outln!(out, "Public:   {cs}");
            }
        }

        DbAction::Delete { name, yes } => {
            out.confirm(
                yes,
                &format!("Delete database {name}? Its container and data volume are removed. This cannot be undone."),
                &format!("delete database {name}"),
            )?;
            let data = client.delete_database(&name).await?;
            out.done("delete", "database", &name, data)?;
            outln!(out, "Database {} deleted", name);
        }

        DbAction::Stop { name } => {
            let data = client.stop_database(&name).await?;
            out.done("stop", "database", &name, data)?;
            outln!(out, "Database {} stopped", name);
        }

        DbAction::Start { name } => {
            let data = client.start_database(&name).await?;
            out.done("start", "database", &name, data)?;
            outln!(out, "Database {} started", name);
        }
    }
    Ok(())
}
