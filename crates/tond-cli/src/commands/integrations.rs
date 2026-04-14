// commands/integrations.rs — `tond integrations` subcommand definitions and handlers

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum IntegrationsAction {
    /// List all notification integrations
    List,

    /// Show details of a specific integration
    Info {
        /// Integration ID
        id: String,
    },

    /// Send a test notification through an integration
    Test {
        /// Integration ID
        id: String,
    },

    /// Enable an integration
    Enable {
        /// Integration ID
        id: String,
    },

    /// Disable an integration
    Disable {
        /// Integration ID
        id: String,
    },

    /// Delete an integration
    Delete {
        /// Integration ID
        id: String,
    },
}

pub async fn handle(client: &TondClient, action: IntegrationsAction) -> Result<()> {
    match action {
        IntegrationsAction::List => {
            let list = client.list_integrations().await?;
            if list.is_empty() {
                println!("No integrations configured.");
                println!("Add Telegram, Discord, Slack, or Email integrations from the dashboard.");
            } else {
                println!("{:<28} {:<10} {:<8} NAME", "ID", "KIND", "ENABLED");
                println!("{}", "-".repeat(72));
                for i in &list {
                    println!(
                        "{:<28} {:<10} {:<8} {}",
                        i.id,
                        i.kind,
                        if i.enabled { "yes" } else { "no" },
                        i.name
                    );
                }
            }
        }

        IntegrationsAction::Info { id } => {
            let i = client.get_integration(&id).await?;
            println!("id:         {}", i.id);
            println!("name:       {}", i.name);
            println!("kind:       {}", i.kind);
            println!("enabled:    {}", if i.enabled { "yes" } else { "no" });
            println!("created:    {}", i.created_at);
            println!("updated:    {}", i.updated_at);
        }

        IntegrationsAction::Test { id } => {
            println!("Sending test notification…");
            let msg = client.test_integration(&id).await?;
            println!("{}", msg);
        }

        IntegrationsAction::Enable { id } => {
            let i = client.set_integration_enabled(&id, true).await?;
            println!("Integration '{}' enabled.", i.name);
        }

        IntegrationsAction::Disable { id } => {
            let i = client.set_integration_enabled(&id, false).await?;
            println!("Integration '{}' disabled.", i.name);
        }

        IntegrationsAction::Delete { id } => {
            client.delete_integration(&id).await?;
            println!("Integration {} deleted.", id);
        }
    }

    Ok(())
}
