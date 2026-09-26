// commands/settings.rs — `tond settings` and `tond passwd` subcommand definitions and handlers

use anyhow::Result;
use clap::Subcommand;
use tond_common::UpdateSettingsRequest;

use crate::client::TondClient;

// Variant names are the subcommand names (`tond settings set-...`).
#[allow(clippy::enum_variant_names)]
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
    /// Set the domain previews are served under (<slug>-<app>.<domain>)
    ///
    /// Must be a separate domain you own — not the dashboard domain, a parent
    /// or subdomain of it, or anything on the same registrable domain — with a
    /// wildcard DNS record *.<domain> pointing at this server. Previews run
    /// pull-request code, so they must not share a site with the dashboard.
    /// Previews cannot be enabled until this is set.
    SetPreviewDomain {
        /// Domain name (e.g. example-previews.com). Pass an empty string to clear.
        domain: String,
    },
    /// Set the domain automatic app hostnames live under (<app>.<domain>)
    ///
    /// Must be a separate site from the dashboard and preview domains (apps
    /// run your code; keeping them off the dashboard's site keeps them away
    /// from its cookies), with a wildcard DNS record *.<domain> pointing at
    /// this server. Without it, apps need a custom domain.
    SetAppsDomain {
        /// Domain name (e.g. example-apps.com). Pass an empty string to clear.
        domain: String,
    },
    /// Turn the dashboard's host terminal (a shell on this server) on or off
    ///
    /// Off by default. It can only be switched from the server itself (this
    /// command, run as root), never from the dashboard or with an API key. When
    /// on, only admins can use it and every session asks for their password
    /// (and 2FA code).
    HostTerminal {
        #[arg(value_enum)]
        state: OnOff,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OnOff {
    On,
    Off,
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
            if s.apps_domain.is_empty() {
                println!("apps-domain       (not set — apps need a custom domain)");
            } else {
                println!("apps-domain       {}", s.apps_domain);
            }
            if s.preview_domain.is_empty() {
                println!("preview-domain    (not set — previews disabled)");
            } else {
                println!("preview-domain    {}", s.preview_domain);
            }
            match client.get_host_terminal().await {
                Ok(true) => println!("host-terminal     on"),
                Ok(false) => println!("host-terminal     off"),
                Err(_) => {}
            }
        }
        Some(SettingsAction::HostTerminal { state }) => {
            let enabled = client.set_host_terminal(state == OnOff::On).await?;
            if enabled {
                println!("host-terminal is on.");
                println!(
                    "Admins can now open a shell on this server from the dashboard terminal; \
                     each session asks for their password (and 2FA code)."
                );
                println!("Turn it off again with: sudo tond settings host-terminal off");
            } else {
                println!("host-terminal is off. Container terminals are still available.");
            }
        }
        Some(SettingsAction::SetServerName { name }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    server_name: Some(name.clone()),
                    ..Default::default()
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
                    dashboard_domain: Some(domain.clone()),
                    ..Default::default()
                })
                .await?;
            if domain.is_empty() {
                println!("dashboard-domain cleared.");
            } else {
                println!("dashboard-domain set to: {}", s.dashboard_domain);
                println!("tond-proxy will pick up the change within 5 seconds.");
            }
        }
        Some(SettingsAction::SetAppsDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    apps_domain: Some(domain.clone()),
                    ..Default::default()
                })
                .await?;
            if domain.is_empty() {
                println!(
                    "apps-domain cleared. New apps get no automatic hostname; give them a custom domain."
                );
            } else {
                println!("apps-domain set to: {}", s.apps_domain);
                println!(
                    "Point a wildcard DNS record *.{} at this server. New apps are served at <app>.{}",
                    s.apps_domain, s.apps_domain
                );
            }
        }
        Some(SettingsAction::SetPreviewDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    preview_domain: Some(domain.clone()),
                    ..Default::default()
                })
                .await?;
            if domain.is_empty() {
                println!("preview-domain cleared. New previews are refused until one is set.");
            } else {
                println!("preview-domain set to: {}", s.preview_domain);
                println!(
                    "Point a wildcard DNS record *.{} at this server.",
                    s.preview_domain
                );
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
