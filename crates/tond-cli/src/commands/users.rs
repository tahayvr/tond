// commands/users.rs — `tond users` subcommands
//
//   tond users list                — dashboard accounts, roles and status
//   tond users reset-link <email>  — one-time set-password link (24 h)
//
// `reset-link` over the root-only socket is the break-glass way back in when
// the only admin has lost their password.

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum UsersAction {
    /// List dashboard users
    List,

    /// Print a one-time link for a user to set a new password (valid 24 hours)
    ResetLink {
        /// The user's email address
        email: String,
    },
}

pub async fn handle(client: &TondClient, action: UsersAction) -> Result<()> {
    match action {
        UsersAction::List => {
            for u in client.list_users().await? {
                let status = if u.disabled {
                    "disabled"
                } else if !u.password_set {
                    "invited"
                } else {
                    "active"
                };
                println!(
                    "{:<32} {:<9} {:<9} 2FA {}  last sign-in {}",
                    u.email,
                    u.role,
                    status,
                    if u.totp_enabled { "on " } else { "off" },
                    u.last_login_at.as_deref().unwrap_or("never")
                );
            }
        }
        UsersAction::ResetLink { email } => {
            let users = client.list_users().await?;
            let user = users
                .iter()
                .find(|u| u.email.eq_ignore_ascii_case(email.trim()))
                .ok_or_else(|| anyhow::anyhow!("no user with the email {email}"))?;
            let link = client.create_password_link(&user.id).await?;
            let domain = client
                .get_settings()
                .await
                .map(|s| s.dashboard_domain)
                .unwrap_or_default();
            let host = if domain.trim().is_empty() {
                "<dashboard-domain>".to_string()
            } else {
                domain
            };
            println!(
                "Send this link to {} (valid until {}, works once):\n\n  https://{}{}\n",
                user.email, link.expires_at, host, link.path
            );
            println!("Setting the password signs the account out everywhere.");
        }
    }
    Ok(())
}
