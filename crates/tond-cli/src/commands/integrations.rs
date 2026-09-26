// commands/integrations.rs — `tond integrations` subcommand definitions and handlers

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::client::TondClient;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum IntegrationsAction {
    /// List all notification integrations
    #[command(visible_alias = "ls")]
    List,

    /// Show details of a specific integration
    #[command(visible_aliases = ["show", "get"])]
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

    /// Delete an integration (asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Integration ID
        id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

pub async fn handle(client: &TondClient, out: &Output, action: IntegrationsAction) -> Result<()> {
    match action {
        IntegrationsAction::List => {
            let list = client.list_integrations().await?;
            out.json(&list)?;
            if list.is_empty() {
                outln!(out, "No integrations configured.");
                outln!(
                    out,
                    "Add Telegram, Discord, Slack, or Email integrations from the dashboard."
                );
            } else {
                outln!(out, "{:<28} {:<10} {:<8} NAME", "ID", "KIND", "ENABLED");
                outln!(out, "{}", "-".repeat(72));
                for i in &list {
                    outln!(
                        out,
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
            out.json(&i)?;
            outln!(out, "id:         {}", i.id);
            outln!(out, "name:       {}", i.name);
            outln!(out, "kind:       {}", i.kind);
            outln!(out, "enabled:    {}", if i.enabled { "yes" } else { "no" });
            outln!(out, "created:    {}", i.created_at);
            outln!(out, "updated:    {}", i.updated_at);
        }

        IntegrationsAction::Test { id } => {
            outln!(out, "Sending test notification…");
            let msg = client.test_integration(&id).await?;
            out.json(&json!({ "ok": true, "id": id, "message": msg }))?;
            outln!(out, "{}", msg);
        }

        IntegrationsAction::Enable { id } => {
            let i = client.set_integration_enabled(&id, true).await?;
            out.json(&i)?;
            outln!(out, "Integration '{}' enabled.", i.name);
        }

        IntegrationsAction::Disable { id } => {
            let i = client.set_integration_enabled(&id, false).await?;
            out.json(&i)?;
            outln!(out, "Integration '{}' disabled.", i.name);
        }

        IntegrationsAction::Delete { id, yes } => {
            out.confirm(
                yes,
                &format!("Delete integration {id}? Its notifications stop."),
                &format!("delete integration {id}"),
            )?;
            let data = client.delete_integration(&id).await?;
            out.done("delete", "integration", &id, data)?;
            outln!(out, "Integration {} deleted.", id);
        }
    }

    Ok(())
}
