// commands/db.rs — `tond db` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum DbAction {
    /// List all managed databases
    List,

    /// Create a new managed database
    Create {
        /// Database name (alphanumeric + hyphens)
        name: String,

        /// Database type: postgres, mysql, redis
        #[arg(short = 't', long)]
        db_type: String,

        /// Project ID to attach to (optional)
        #[arg(long)]
        project: Option<String>,

        /// Memory limit in megabytes (default: 256)
        #[arg(long)]
        memory_mb: Option<i64>,
    },

    /// Show details for a managed database
    Info {
        /// Database name or ID
        name: String,
    },

    /// Delete a managed database (stops container and removes volume)
    Delete {
        /// Database name or ID
        name: String,

        /// Skip confirmation
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

pub async fn handle(client: &TondClient, action: DbAction) -> Result<()> {
    match action {
        DbAction::List => {
            let dbs = client.list_databases().await?;
            if dbs.is_empty() {
                println!(
                    "No managed databases yet. Create one with: tond db create <name> --type <postgres|mysql|redis>"
                );
                return Ok(());
            }
            println!(
                "{:<20} {:<10} {:<8} {:<10} {:<26}",
                "NAME", "TYPE", "STATUS", "PORT", "CREATED"
            );
            println!("{}", "-".repeat(74));
            for db in &dbs {
                println!(
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
            let db = client
                .create_database(&name, &db_type, project.as_deref(), memory_limit)
                .await?;
            println!(
                "Database created: {} (ID: {})",
                db.database.name, db.database.id
            );
            println!("Type:   {}", db.database.db_type);
            println!("Status: {}", db.database.status);
            if let Some(port) = db.database.host_port {
                println!("Port:   {}", port);
            }
            if let Some(ref cs) = db.connection_string {
                println!("Conn:   {}", cs);
            }
        }

        DbAction::Info { name } => {
            let db = client.get_database(&name).await?;
            println!("Name:    {}", db.database.name);
            println!("ID:      {}", db.database.id);
            println!("Type:    {}", db.database.db_type);
            println!("Status:  {}", db.database.status);
            println!(
                "Project: {}",
                db.database.project_id.as_deref().unwrap_or("-")
            );
            match db.database.host_port {
                Some(p) => println!("Port:    {}", p),
                None => println!("Port:    -"),
            }
            match db.database.memory_limit {
                Some(bytes) => println!("Memory:  {} MiB", bytes / 1_048_576),
                None => println!("Memory:  default"),
            }
            if let Some(ref cs) = db.connection_string {
                println!("Conn:    {}", cs);
            }
            if let Some(ref ics) = db.internal_connection_string {
                println!("Internal:{}", ics);
            }
            println!("Created: {}", db.database.created_at);
        }

        DbAction::Delete { name, yes } => {
            if !yes {
                print!("Are you sure you want to delete database {}? [y/N] ", name);
                use std::io::{self, Write};
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() != "y" {
                    println!("Cancelled");
                    return Ok(());
                }
            }
            client.delete_database(&name).await?;
            println!("Database {} deleted", name);
        }

        DbAction::Stop { name } => {
            client.stop_database(&name).await?;
            println!("Database {} stopped", name);
        }

        DbAction::Start { name } => {
            client.start_database(&name).await?;
            println!("Database {} started", name);
        }
    }
    Ok(())
}
