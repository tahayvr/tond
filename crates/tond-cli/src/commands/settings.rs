// commands/settings.rs — `tond settings` and `tond passwd` subcommand definitions and handlers

use anyhow::Result;
use clap::Subcommand;
use tond_common::UpdateSettingsRequest;

use crate::client::TondClient;
use crate::error::CliError;
use crate::output::{Output, outln};

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
    /// command, run as root), never from the dashboard or with an API token. When
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
pub async fn handle(
    client: &TondClient,
    out: &Output,
    action: Option<SettingsAction>,
) -> Result<()> {
    match action {
        None => {
            let s = client.get_settings().await?;
            if s.server_name.is_empty() {
                outln!(out, "server-name       (not set)");
            } else {
                outln!(out, "server-name       {}", s.server_name);
            }
            if s.dashboard_domain.is_empty() {
                outln!(out, "dashboard-domain  (not set)");
            } else {
                outln!(out, "dashboard-domain  {}", s.dashboard_domain);
            }
            if s.apps_domain.is_empty() {
                outln!(
                    out,
                    "apps-domain       (not set — apps need a custom domain)"
                );
            } else {
                outln!(out, "apps-domain       {}", s.apps_domain);
            }
            if s.preview_domain.is_empty() {
                outln!(out, "preview-domain    (not set — previews disabled)");
            } else {
                outln!(out, "preview-domain    {}", s.preview_domain);
            }
            let host_terminal = client.get_host_terminal().await.ok();
            match host_terminal {
                Some(true) => outln!(out, "host-terminal     on"),
                Some(false) => outln!(out, "host-terminal     off"),
                None => {}
            }
            let mut doc = serde_json::to_value(&s)?;
            doc["host_terminal"] = serde_json::json!(host_terminal);
            out.json(&doc)?;
        }
        Some(SettingsAction::HostTerminal { state }) => {
            let enabled = client.set_host_terminal(state == OnOff::On).await?;
            out.json(&serde_json::json!({ "host_terminal": enabled }))?;
            if enabled {
                outln!(out, "host-terminal is on.");
                outln!(
                    out,
                    "Admins can now open a shell on this server from the dashboard terminal; \
                     each session asks for their password (and 2FA code)."
                );
                outln!(
                    out,
                    "Turn it off again with: sudo tond settings host-terminal off"
                );
            } else {
                outln!(
                    out,
                    "host-terminal is off. Container terminals are still available."
                );
            }
        }
        Some(SettingsAction::SetServerName { name }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    server_name: Some(name.clone()),
                    dashboard_domain: None,
                    preview_domain: None,
                    ..UpdateSettingsRequest::default()
                })
                .await?;
            out.json(&s)?;
            if name.is_empty() {
                outln!(out, "server-name cleared.");
            } else {
                outln!(out, "server-name set to: {}", s.server_name);
            }
        }
        Some(SettingsAction::SetDashboardDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    dashboard_domain: Some(domain.clone()),
                    preview_domain: None,
                    ..UpdateSettingsRequest::default()
                })
                .await?;
            out.json(&s)?;
            if domain.is_empty() {
                outln!(out, "dashboard-domain cleared.");
            } else {
                outln!(out, "dashboard-domain set to: {}", s.dashboard_domain);
                outln!(out, "tond-proxy will pick up the change within 5 seconds.");
            }
        }
        Some(SettingsAction::SetAppsDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    apps_domain: Some(domain.clone()),
                    ..Default::default()
                })
                .await?;
            out.json(&s)?;
            if domain.is_empty() {
                outln!(
                    out,
                    "apps-domain cleared. New apps get no automatic hostname; give them a custom domain."
                );
            } else {
                outln!(out, "apps-domain set to: {}", s.apps_domain);
                outln!(
                    out,
                    "Point a wildcard DNS record *.{} at this server. New apps are served at <app>.{}",
                    s.apps_domain,
                    s.apps_domain
                );
            }
        }
        Some(SettingsAction::SetPreviewDomain { domain }) => {
            let s = client
                .update_settings(UpdateSettingsRequest {
                    preview_domain: Some(domain.clone()),
                    ..UpdateSettingsRequest::default()
                })
                .await?;
            out.json(&s)?;
            if domain.is_empty() {
                outln!(
                    out,
                    "preview-domain cleared. New previews are refused until one is set."
                );
            } else {
                outln!(out, "preview-domain set to: {}", s.preview_domain);
                outln!(
                    out,
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
/// Note: The change-password endpoint needs a dashboard session; API tokens
/// and the root socket can't change a user's password. Root on the host can
/// instead issue a one-time reset link: `tond users reset-link <email>`.
/// Input is read from stdin (characters are visible; pipe a password
/// manager's output for non-interactive use).
pub async fn handle_passwd(client: &TondClient, out: &Output) -> Result<()> {
    let current = out.prompt_line("Current password: ")?;
    let new_pw = out.prompt_line("New password: ")?;

    if new_pw.chars().count() < 12 {
        return Err(CliError::usage("New password must be at least 12 characters").into());
    }

    let confirm = out.prompt_line("Confirm new password: ")?;

    if new_pw != confirm {
        return Err(CliError::usage("Passwords do not match").into());
    }

    client.change_password(&current, &new_pw).await?;
    out.json(&serde_json::json!({ "ok": true, "action": "change_password" }))?;
    outln!(out, "Password changed successfully.");
    outln!(out, "All other active sessions have been signed out.");
    Ok(())
}
