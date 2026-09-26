// commands/storage_providers.rs — `tond storage-provider` subcommand

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::client::TondClient;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum StorageProviderAction {
    /// List all configured storage providers
    #[command(visible_alias = "ls")]
    List,

    /// Create a new S3-compatible storage provider
    Create {
        /// Human-readable label for this provider
        #[arg(long)]
        name: String,

        /// S3-compatible endpoint URL (e.g. https://s3.amazonaws.com)
        #[arg(long)]
        endpoint: String,

        /// AWS region (e.g. us-east-1)
        #[arg(long)]
        region: String,

        /// Bucket name
        #[arg(long)]
        bucket: String,

        /// Key prefix inside the bucket (optional)
        #[arg(long)]
        prefix: Option<String>,

        /// Access key ID
        #[arg(long)]
        key_id: String,

        /// Secret access key
        #[arg(long)]
        secret: String,
    },

    /// Test connectivity to a storage provider
    Test {
        /// Storage provider ID
        id: String,
    },

    /// Delete a storage provider (asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Storage provider ID
        id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

pub async fn handle(
    client: &TondClient,
    out: &Output,
    action: StorageProviderAction,
) -> Result<()> {
    match action {
        StorageProviderAction::List => {
            let list = client.list_storage_providers().await?;
            out.json(&list)?;
            if list.is_empty() {
                outln!(out, "No storage providers configured.");
                outln!(out, "Use `tond storage-provider create` to add one.");
            } else {
                outln!(out, "{:<28} {:<6} NAME", "ID", "KIND");
                outln!(out, "{}", "-".repeat(60));
                for p in &list {
                    outln!(out, "{:<28} {:<6} {}", p.id, p.kind, p.name);
                }
            }
        }

        StorageProviderAction::Create {
            name,
            endpoint,
            region,
            bucket,
            prefix,
            key_id,
            secret,
        } => {
            let p = client
                .create_storage_provider(
                    &name,
                    &endpoint,
                    &region,
                    &bucket,
                    prefix.as_deref(),
                    &key_id,
                    &secret,
                )
                .await?;
            out.json(&p)?;
            outln!(out, "Storage provider created: {} ({})", p.name, p.id);
        }

        StorageProviderAction::Test { id } => {
            outln!(out, "Testing connectivity…");
            let msg = client.test_storage_provider(&id).await?;
            out.json(&json!({ "ok": true, "id": id, "message": msg }))?;
            outln!(out, "{}", msg);
        }

        StorageProviderAction::Delete { id, yes } => {
            out.confirm(
                yes,
                &format!(
                    "Delete storage provider {id}? Snapshots stored there are no longer reachable from Tond."
                ),
                &format!("delete storage provider {id}"),
            )?;
            let data = client.delete_storage_provider(&id).await?;
            out.done("delete", "storage_provider", &id, data)?;
            outln!(out, "Storage provider {} deleted.", id);
        }
    }

    Ok(())
}
