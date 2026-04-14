// commands/storage_providers.rs — `tond storage-provider` subcommand

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum StorageProviderAction {
    /// List all configured storage providers
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

    /// Delete a storage provider
    Delete {
        /// Storage provider ID
        id: String,
    },
}

pub async fn handle(client: &TondClient, action: StorageProviderAction) -> Result<()> {
    match action {
        StorageProviderAction::List => {
            let list = client.list_storage_providers().await?;
            if list.is_empty() {
                println!("No storage providers configured.");
                println!("Use `tond storage-provider create` to add one.");
            } else {
                println!("{:<28} {:<6} NAME", "ID", "KIND");
                println!("{}", "-".repeat(60));
                for p in &list {
                    println!("{:<28} {:<6} {}", p.id, p.kind, p.name);
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
            println!("Storage provider created: {} ({})", p.name, p.id);
        }

        StorageProviderAction::Test { id } => {
            println!("Testing connectivity…");
            let msg = client.test_storage_provider(&id).await?;
            println!("{}", msg);
        }

        StorageProviderAction::Delete { id } => {
            client.delete_storage_provider(&id).await?;
            println!("Storage provider {} deleted.", id);
        }
    }

    Ok(())
}
