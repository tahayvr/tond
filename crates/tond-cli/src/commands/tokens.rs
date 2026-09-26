// commands/tokens.rs — `tond token` subcommands
//
// `tond token create` over the root-only socket is how automation is
// bootstrapped (there is no root API key):
//
//   sudo tond token create --name ci --preset deployer --app web --expires 30d
//
// The secret is printed once, on stdout alone, so it can be captured with
// `TOKEN=$(sudo tond token create … )`; everything else goes to stderr.
// With --json, stdout holds the whole created-token object (secret included).

use anyhow::Result;
use clap::Subcommand;
use tond_common::{CreateTokenRequest, TokenKind, TokenResponse};

use crate::client::TondClient;
use crate::error::CliError;
use crate::output::{Output, note, outln};

#[derive(Subcommand)]
pub enum TokenAction {
    /// Create an API token and print it once
    Create {
        /// What the token is for (e.g. the CI workflow or machine)
        #[arg(long)]
        name: String,

        /// personal, service, agent or ci
        #[arg(long, default_value = "service")]
        kind: String,

        /// Start from a preset: readonly, deployer, agent-operator
        #[arg(long)]
        preset: Option<String>,

        /// Extra scope, e.g. apps:deploy (repeatable)
        #[arg(long = "scope")]
        scopes: Vec<String>,

        /// Limit the token to this app, by name or id (repeatable)
        #[arg(long = "app")]
        apps: Vec<String>,

        /// Limit the token to the apps of this project (repeatable)
        #[arg(long = "project")]
        projects: Vec<String>,

        /// Only accept the token from this IP or CIDR range (repeatable)
        #[arg(long = "allow-ip")]
        allowed_ips: Vec<String>,

        /// Lifetime such as 24h, 30d or 12w, or "never" (agent tokens must
        /// expire). Default: 24h for agent tokens, 90d otherwise.
        #[arg(long)]
        expires: Option<String>,
    },

    /// List API tokens (yours, or every token with --all)
    #[command(visible_alias = "ls")]
    List {
        /// Every user's tokens (needs tokens:admin; always the case as root)
        #[arg(long)]
        all: bool,
    },

    /// Revoke an API token (by id, name or visible prefix; asks first)
    #[command(visible_aliases = ["delete", "rm"])]
    Revoke {
        token: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// Parse a lifetime like `24h`, `30d`, `12w` or `never` into hours (0 = never).
pub fn parse_expiry(value: &str) -> Result<i64> {
    let value = value.trim().to_lowercase();
    if value == "never" || value == "0" {
        return Ok(0);
    }
    let (num, unit) = value.split_at(value.len().saturating_sub(1));
    let n: i64 = num.parse().map_err(|_| {
        anyhow::anyhow!("invalid lifetime '{value}': use e.g. 24h, 30d, 12w or never")
    })?;
    if n <= 0 {
        anyhow::bail!("the lifetime must be positive");
    }
    Ok(match unit {
        "h" => n,
        "d" => n * 24,
        "w" => n * 24 * 7,
        _ => anyhow::bail!("invalid lifetime '{value}': use e.g. 24h, 30d, 12w or never"),
    })
}

fn describe(t: &TokenResponse) -> String {
    let scopes: Vec<String> = t.scopes.iter().map(|s| s.to_string()).collect();
    let mut limits = Vec::new();
    if !t.app_ids.is_empty() {
        limits.push(format!("apps {}", t.app_ids.join(",")));
    }
    if !t.project_ids.is_empty() {
        limits.push(format!("projects {}", t.project_ids.join(",")));
    }
    if !t.allowed_ips.is_empty() {
        limits.push(format!("from {}", t.allowed_ips.join(",")));
    }
    let status = if t.revoked_at.is_some() {
        "revoked".to_string()
    } else {
        match &t.expires_at {
            Some(e) => format!("expires {e}"),
            None => "never expires".to_string(),
        }
    };
    format!(
        "{:<20} {:<18} {:<9} {:<24} {}\n    scopes: {}{}\n    last used: {}",
        t.name,
        t.prefix,
        t.kind,
        t.owner_label,
        status,
        scopes.join(" "),
        if limits.is_empty() {
            String::new()
        } else {
            format!("\n    limited to: {}", limits.join("; "))
        },
        match (&t.last_used_at, &t.last_used_ip) {
            (Some(at), Some(ip)) => format!("{at} from {ip}"),
            (Some(at), None) => at.clone(),
            _ => "never".to_string(),
        }
    )
}

pub async fn handle(client: &TondClient, out: &Output, action: TokenAction) -> Result<()> {
    match action {
        TokenAction::Create {
            name,
            kind,
            preset,
            scopes,
            apps,
            projects,
            allowed_ips,
            expires,
        } => {
            let kind: TokenKind = kind.parse().map_err(CliError::usage)?;
            let expires_in_hours = expires
                .as_deref()
                .map(parse_expiry)
                .transpose()
                .map_err(|e| CliError::usage(e.to_string()))?;
            let created = client
                .create_token(&CreateTokenRequest {
                    name,
                    kind: Some(kind),
                    preset,
                    scopes,
                    app_ids: apps,
                    project_ids: projects,
                    allowed_ips,
                    expires_in_hours,
                })
                .await?;
            out.json(&created)?;
            note!(out, "Created API token:\n{}", describe(&created.token));
            note!(out, "\nStore it now: it won't be shown again.");
            outln!(out, "{}", created.secret);
        }
        TokenAction::List { all } => {
            let tokens = client.list_tokens(all).await?;
            out.json(&tokens)?;
            if tokens.is_empty() {
                outln!(
                    out,
                    "No API tokens. Create one with `tond token create --name <name> --preset readonly`."
                );
            }
            for t in tokens {
                outln!(out, "{}  (id {})", describe(&t), t.id);
            }
        }
        TokenAction::Revoke { token, yes } => {
            out.can_confirm(yes, &format!("revoke token {token}"))?;
            let tokens = client.list_tokens(true).await?;
            let matches: Vec<&TokenResponse> = tokens
                .iter()
                .filter(|t| t.id == token || t.prefix == token || t.name == token)
                .filter(|t| t.revoked_at.is_none())
                .collect();
            let target = match matches.as_slice() {
                [one] => *one,
                [] => {
                    return Err(CliError::coded(
                        tond_common::code::NOT_FOUND,
                        tond_common::exit::NOT_FOUND,
                        format!("no active token matches '{token}'"),
                    )
                    .with_hint("See `tond token list --all`.")
                    .into());
                }
                _ => {
                    return Err(CliError::usage(format!(
                        "'{token}' matches several tokens; use the id from `tond token list --all`"
                    ))
                    .into());
                }
            };
            out.confirm(
                yes,
                &format!(
                    "Revoke token {} ({})? Anything using it stops working.",
                    target.name, target.prefix
                ),
                &format!("revoke token {}", target.name),
            )?;
            let revoked = client.revoke_token(&target.id).await?;
            out.json(&revoked)?;
            outln!(out, "Revoked {} ({}).", target.name, target.prefix);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetimes_parse_into_hours() {
        assert_eq!(parse_expiry("24h").ok(), Some(24));
        assert_eq!(parse_expiry("30d").ok(), Some(720));
        assert_eq!(parse_expiry("2w").ok(), Some(336));
        assert_eq!(parse_expiry("never").ok(), Some(0));
        assert!(parse_expiry("30").is_err());
        assert!(parse_expiry("-1d").is_err());
        assert!(parse_expiry("abc").is_err());
    }
}
