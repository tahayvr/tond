// commands/settings.rs — `tond settings` and `tond passwd` subcommand definitions and handlers

use anyhow::Result;
use clap::Subcommand;
use tond_common::UpdateSettingsRequest;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum SettingsAction {
    /// Set the human-readable name for this Tond server instance
    SetServerName {
        /// Server name (e.g. "My VPS"). Pass an empty string to clear.
        name: String,
    },
    /// Set the domain used to access the Tond dashboard
    ///
    /// The domain is written to the database. tond-proxy picks it up within
    /// 5 seconds — no proxy restart needed.
    SetDashboardDomain {
        /// Domain name (e.g. tond.example.com). Pass an empty string to clear.
        domain: String,
    },
}

/// Handle `tond settings [subcommand]`.
///
/// With no subcommand, prints the current settings.
/// Subcommands allow updating individual settings.
pub async fn handle(client: &TondClient, action: Option<SettingsAction>) -> Result<()> {
    match action {
        None => {
            let s = client.get_settings().await?;
            if s.server_name.is_empty() {
                println!("server-name       (not set)");
            } else {
                println!("server-name       {}", s.server_name);
            }
            if s.dashboard_domain.is_empty() {
                println!("dashboard-domain  (not set)");
            } else {
                println!("dashboard-domain  {}", s.dashboard_domain);
            }
        }
        Some(SettingsAction::SetServerName { name }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    server_name: Some(name.clone()),
                    dashboard_domain: None,
                })
                .await?;
            if name.is_empty() {
                println!("server-name cleared.");
            } else {
                println!("server-name set to: {}", s.server_name);
            }
        }
        Some(SettingsAction::SetDashboardDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    server_name: None,
                    dashboard_domain: Some(domain.clone()),
                })
                .await?;
            if domain.is_empty() {
                println!("dashboard-domain cleared.");
            } else {
                println!("dashboard-domain set to: {}", s.dashboard_domain);
                println!("tond-proxy will pick up the change within 5 seconds.");
            }
        }
    }
    Ok(())
}

/// Interactive password change flow for `tond passwd`.
/// Prompts for current password and new password (+ confirmation),
/// then calls POST /api/auth/change-password.
///
/// Note: The change-password endpoint uses a session cookie for auth.
/// The CLI sends the API key in the Authorization header — the server
/// accepts both. Input is read from stdin (characters are visible; pipe
/// a password manager's output for non-interactive use).
pub async fn handle_passwd(client: &TondClient) -> Result<()> {
    use std::io::{self, Write};

    let current = read_line_prompt("Current password: ")?;
    let new_pw = read_line_prompt("New password: ")?;

    if new_pw.len() < 8 {
        anyhow::bail!("New password must be at least 8 characters");
    }

    let confirm = read_line_prompt("Confirm new password: ")?;

    if new_pw != confirm {
        anyhow::bail!("Passwords do not match");
    }

    client.change_password(&current, &new_pw).await?;
    println!("Password changed successfully.");
    println!("All other active sessions have been signed out.");
    let _ = io::stdout().flush();
    Ok(())
}

/// Print a prompt and read one line from stdin (trims trailing newline).
pub fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim_end_matches('\n').trim_end_matches('\r').to_string())
}
