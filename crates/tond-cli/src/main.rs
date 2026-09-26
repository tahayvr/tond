// main.rs — tond CLI entry point and command definitions
//
// The `tond` binary is the operator's interface to the platform. It
// communicates with tond-server entirely over HTTP using the typed
// TondClient in client.rs.
//
// Connection and authentication:
//   By default the CLI talks to tond-server over its local Unix socket
//   (--socket / TOND_SOCKET_PATH, default /opt/tond/data/tond.sock). The socket
//   is root-only (0600) and requests on it need no credentials, so run the CLI
//   on the server as root (e.g. over SSH).
//   To reach a server over HTTP(S) instead, pass --server / TOND_SERVER and an
//   API token via --token / TOND_TOKEN. Loopback TCP is not trusted.
//
// Command summary:
//   apps <subcommand>              — manage apps (list/create/deploy/push/info/logs/…)
//   projects <subcommand>          — manage projects (list/create/info/delete/deploy/assign)
//   db <subcommand>                — manage databases (list/create/info/delete/stop/start)
//   cron <subcommand>              — manage cron jobs (list/create/info/update/delete/trigger/runs)
//   integrations <subcommand>      — manage notification integrations (list/info/test/enable/disable/delete)
//   settings [subcommand]          — view or update server-wide settings
//                                    (incl. `settings host-terminal on|off`)
//   token <subcommand>             — API tokens (create/list/revoke); `create` over
//                                    the socket bootstraps automation
//   audit <subcommand>             — audit log (verify the hash chain, show entries)
//   users <subcommand>             — list users, print a one-time password reset link
//   setup-link                     — print the first-run admin setup link again
//   update [options]               — update tond-server to a newer version
//   passwd                         — change dashboard account password

mod client;
mod commands;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use client::TondClient;
use commands::{
    apps::AppsAction, audit::AuditAction, cron::CronAction, db::DbAction,
    integrations::IntegrationsAction, projects::ProjectsAction, scanner::ScannerAction,
    settings::SettingsAction, snapshots::SnapshotAction, storage_providers::StorageProviderAction,
    tokens::TokenAction, users::UsersAction,
};

#[derive(Parser)]
#[command(name = "tond", about = "Tond CLI - Deploy apps with ease", version)]
struct Cli {
    /// Server URL to connect to over HTTP(S) instead of the local socket (requires --token)
    #[arg(long, global = true, env = "TOND_SERVER")]
    server: Option<String>,

    /// API token (tond_pat_…) for --server connections
    #[arg(long, global = true, env = "TOND_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Path to tond-server's local Unix socket (used when --server is not set)
    #[arg(
        long,
        global = true,
        env = "TOND_SOCKET_PATH",
        default_value = tond_common::DEFAULT_SOCKET_PATH
    )]
    socket: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage apps
    Apps {
        #[command(subcommand)]
        action: AppsAction,
    },

    /// Manage projects (app groups)
    Projects {
        #[command(subcommand)]
        action: ProjectsAction,
    },

    /// Manage managed databases (postgres / mysql / redis containers)
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// Manage cron jobs (scheduled container runs)
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },

    /// Manage notification integrations (Telegram, Discord, Slack, Email)
    Integrations {
        #[command(subcommand)]
        action: IntegrationsAction,
    },

    /// Inspect TBuild detection and build-plan decisions for a repository
    Scanner {
        #[command(subcommand)]
        action: ScannerAction,
    },

    /// Update Tond server to the latest version
    Update {
        /// Update to specific version, without the v prefix (default: latest)
        #[arg(short, long)]
        version: Option<String>,

        /// Preview changes without applying them
        #[arg(long)]
        dry_run: bool,

        /// Skip database backup (not recommended)
        #[arg(long)]
        skip_backup: bool,

        /// Force update even if already on latest version
        #[arg(long)]
        force: bool,
    },

    /// View or update server-wide settings
    Settings {
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },

    /// Manage S3-compatible storage providers for backups
    StorageProvider {
        #[command(subcommand)]
        action: StorageProviderAction,
    },

    /// Manage snapshots and backup configuration
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },

    /// Manage API tokens (scoped, expiring credentials for automation)
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },

    /// Manage dashboard users (list, one-time password reset links)
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },

    /// Inspect the audit log
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },

    /// Change your dashboard account password (prompts for current and new password)
    Passwd,

    /// Print the one-time link for creating the first admin account (run as root)
    SetupLink,
}

#[tokio::main]
async fn main() -> Result<()> {
    tond_common::install_crypto_provider();
    let cli = Cli::parse();

    // Connect lazily: `scanner` and `update` don't use the API, so they must work
    // without a reachable server.
    let connect = || match cli.server.as_deref() {
        Some(url) => Ok(TondClient::new(url, cli.token.as_deref())),
        None => TondClient::local(&cli.socket),
    };

    match cli.command {
        Commands::Apps { action } => commands::apps::handle(&connect()?, action).await?,
        Commands::Projects { action } => commands::projects::handle(&connect()?, action).await?,
        Commands::Db { action } => commands::db::handle(&connect()?, action).await?,
        Commands::Cron { action } => commands::cron::handle(&connect()?, action).await?,
        Commands::Integrations { action } => {
            commands::integrations::handle(&connect()?, action).await?
        }
        Commands::Scanner { action } => commands::scanner::handle(action).await?,

        Commands::Update {
            version,
            dry_run,
            skip_backup,
            force,
        } => commands::update::handle(version, dry_run, skip_backup, force)?,

        Commands::Settings { action } => commands::settings::handle(&connect()?, action).await?,
        Commands::StorageProvider { action } => {
            commands::storage_providers::handle(&connect()?, action).await?
        }
        Commands::Snapshot { action } => commands::snapshots::handle(&connect()?, action).await?,
        Commands::Token { action } => commands::tokens::handle(&connect()?, action).await?,
        Commands::Audit { action } => commands::audit::handle(&connect()?, action).await?,
        Commands::Users { action } => commands::users::handle(&connect()?, action).await?,
        Commands::Passwd => commands::settings::handle_passwd(&connect()?).await?,
        Commands::SetupLink => {
            let data_dir = cli
                .socket
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/opt/tond/data"));
            commands::setup::handle_setup_link(connect(), &data_dir).await?
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn cli_uses_default_server_for_simple_command() {
        let cli = Cli::try_parse_from(["tond", "apps", "list"])
            .unwrap_or_else(|err| panic!("failed to parse apps list command: {err}"));

        assert_eq!(
            cli.server, None,
            "CLI should default to the local Unix socket rather than a server URL"
        );
        assert_eq!(
            cli.socket,
            PathBuf::from(tond_common::DEFAULT_SOCKET_PATH),
            "CLI should default to the standard socket path"
        );
        assert!(
            matches!(
                cli.command,
                Commands::Apps {
                    action: AppsAction::List
                }
            ),
            "apps list should parse into the Apps::List subcommand"
        );
    }

    #[test]
    fn cli_allows_overriding_server_url() {
        let cli = Cli::try_parse_from(["tond", "--server", "https://tond.example.com", "passwd"])
            .unwrap_or_else(|err| {
                panic!("failed to parse passwd command with server override: {err}")
            });

        assert_eq!(
            cli.server.as_deref(),
            Some("https://tond.example.com"),
            "CLI should honor the explicit --server override"
        );
        assert!(
            matches!(cli.command, Commands::Passwd),
            "passwd should parse into the Passwd command"
        );
    }

    #[test]
    fn cli_parses_update_flags_and_version() {
        let cli = Cli::try_parse_from([
            "tond",
            "update",
            "--version",
            "1.2.3",
            "--dry-run",
            "--skip-backup",
            "--force",
        ])
        .unwrap_or_else(|err| panic!("failed to parse update command: {err}"));

        match cli.command {
            Commands::Update {
                version,
                dry_run,
                skip_backup,
                force,
            } => {
                assert_eq!(
                    version,
                    Some("1.2.3".to_string()),
                    "update should preserve the requested target version"
                );
                assert!(dry_run, "update should parse --dry-run as true");
                assert!(skip_backup, "update should parse --skip-backup as true");
                assert!(force, "update should parse --force as true");
            }
            _ => panic!("expected Update command"),
        }
    }

    #[test]
    fn cli_parses_settings_without_subcommand() {
        let cli = Cli::try_parse_from(["tond", "settings"])
            .unwrap_or_else(|err| panic!("failed to parse settings command: {err}"));

        match cli.command {
            Commands::Settings { action } => {
                assert!(
                    action.is_none(),
                    "settings without a nested command should parse with no action"
                );
            }
            _ => panic!("expected Settings command"),
        }
    }

    #[test]
    fn cli_parses_apps_create_defaults() {
        let cli = Cli::try_parse_from(["tond", "apps", "create", "demo"])
            .unwrap_or_else(|err| panic!("failed to parse apps create command: {err}"));

        match cli.command {
            Commands::Apps {
                action: AppsAction::Create(create),
            } => {
                let create = *create;
                assert_eq!(
                    create.name, "demo",
                    "apps create should preserve the app name"
                );
                assert_eq!(
                    create.template, None,
                    "apps create should default template to None"
                );
                assert_eq!(
                    create.source_type, "git",
                    "apps create should default source_type to git"
                );
                assert_eq!(
                    create.source, None,
                    "apps create should default source to None"
                );
                assert_eq!(
                    create.branch, "main",
                    "apps create should default branch to main"
                );
                assert_eq!(create.port, 3000, "apps create should default port to 3000");
                assert_eq!(
                    create.domain, None,
                    "apps create should default domain to None"
                );
                assert!(
                    create.env.is_empty(),
                    "apps create should default env overrides to an empty list"
                );
                assert_eq!(
                    create.memory_mb, None,
                    "apps create should default memory_mb to None"
                );
                assert_eq!(create.cpus, None, "apps create should default cpus to None");
                assert_eq!(
                    create.health_path, None,
                    "apps create should default health_path to None"
                );
                assert_eq!(
                    create.health_interval, None,
                    "apps create should default health_interval to None"
                );
                assert_eq!(
                    create.health_threshold, None,
                    "apps create should default health_threshold to None"
                );
                assert_eq!(
                    create.drain_timeout, None,
                    "apps create should default drain_timeout to None"
                );
                assert_eq!(
                    create.build_method, None,
                    "apps create should leave build_method to the server default"
                );
                assert!(!create.yes, "apps create should default yes to false");
            }
            _ => panic!("expected Apps::Create command"),
        }
    }

    #[test]
    fn cli_parses_scanner_explain_with_base_dir_and_static_build() {
        let cli = Cli::try_parse_from([
            "tond",
            "scanner",
            "explain",
            "/tmp/repo",
            "--base-dir",
            "apps/web",
            "--static-build",
        ])
        .unwrap_or_else(|err| panic!("failed to parse scanner explain command: {err}"));

        match cli.command {
            Commands::Scanner {
                action: ScannerAction::Explain(options),
            } => {
                assert_eq!(
                    options.path, "/tmp/repo",
                    "scanner explain should preserve the target repository path"
                );
                assert_eq!(
                    options.base_dir,
                    Some("apps/web".to_string()),
                    "scanner explain should parse --base-dir"
                );
                assert!(
                    options.static_build,
                    "scanner explain should parse --static-build"
                );
            }
            _ => panic!("expected Scanner::Explain command"),
        }
    }

    #[test]
    fn cli_parses_apps_create_with_template() {
        let cli = Cli::try_parse_from([
            "tond",
            "apps",
            "create",
            "analytics",
            "--template",
            "plausible",
            "--env",
            "BASE_URL=https://stats.example.com",
            "--yes",
        ])
        .unwrap_or_else(|err| panic!("failed to parse templated apps create command: {err}"));

        match cli.command {
            Commands::Apps {
                action: AppsAction::Create(create),
            } => {
                let create = *create;
                assert_eq!(
                    create.name, "analytics",
                    "templated create should preserve the app name"
                );
                assert_eq!(
                    create.template.as_deref(),
                    Some("plausible"),
                    "templated create should parse the selected template id"
                );
                assert_eq!(
                    create.env,
                    vec!["BASE_URL=https://stats.example.com".to_string()],
                    "templated create should collect repeated --env values"
                );
                assert!(create.yes, "templated create should parse --yes");
            }
            _ => panic!("expected Apps::Create command"),
        }
    }

    #[test]
    fn cli_rejects_snapshot_rollback_with_both_targets() {
        let result = Cli::try_parse_from([
            "tond",
            "snapshot",
            "rollback",
            "snap-1",
            "--app",
            "web",
            "--database",
            "db",
        ]);

        assert!(
            result.is_err(),
            "snapshot rollback should reject using both --app and --database together"
        );
    }
}
