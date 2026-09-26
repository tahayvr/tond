// main.rs — tond CLI entry point and command definitions
//
// The `tond` binary is the operator's interface to the platform. It
// communicates with tond-server entirely over HTTP (/api/v1) using the typed
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
// Automation:
//   --json                 machine-readable output on stdout: one JSON document
//                          (list commands print arrays), or NDJSON events for
//                          `apps logs --follow` and `apps deploy/push` (ending
//                          with a `result` event). Errors go to stderr as one
//                          line {"error":{"code","message","hint","field"},"exit_code":N}.
//   --idempotency-key K    Idempotency-Key for mutating requests (default: a
//                          fresh UUID per request); transient failures are
//                          retried with the same key.
//   --yes / -y             required by destructive commands when stdin is not
//                          a terminal (otherwise they refuse with exit 2).
//   Exit codes (`tond help exit-codes`): 0 ok, 1 error, 2 usage, 3 not found,
//   4 denied, 5 conflict, 6 build/deploy failed, 7 approval required,
//   8 server unavailable.
//
// Command summary:
//   apps <subcommand>              — manage apps (list/create/deploy/push/info/logs/…);
//                                    `deploy`/`push` wait for the build (--no-wait to return)
//   projects <subcommand>          — manage projects (list/create/info/delete/deploy/assign)
//   db <subcommand>                — manage databases (list/create/info/delete/stop/start)
//   cron <subcommand>              — manage cron jobs (list/create/info/update/delete/trigger/runs)
//   integrations <subcommand>      — manage notification integrations (list/info/test/enable/disable/delete)
//   settings [subcommand]          — view or update server-wide settings
//                                    (incl. `settings host-terminal on|off`)
//   storage-provider <subcommand>  — S3-compatible backup targets
//   snapshot <subcommand>          — snapshots, restores and backup configuration
//   token <subcommand>             — API tokens (create/list/revoke); `create` over
//                                    the socket bootstraps automation
//   audit <subcommand>             — audit log (verify the hash chain, show entries)
//   users <subcommand>             — list users, print a one-time password reset link
//   scanner explain <path>         — show how TBuild would build a repository
//   setup-link                     — print the first-run admin setup link again
//   update [options]               — update tond-server to a newer version
//   passwd                         — change dashboard account password
//   completions <shell>            — shell completion script (bash, zsh, fish, …)
//   help [command…|exit-codes]     — help for a command, or the exit-code table

mod client;
mod commands;
mod deploy_follow;
mod error;
mod output;
mod signature;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use client::TondClient;
use commands::{
    apps::AppsAction, audit::AuditAction, cron::CronAction, db::DbAction,
    integrations::IntegrationsAction, projects::ProjectsAction, scanner::ScannerAction,
    settings::SettingsAction, snapshots::SnapshotAction, storage_providers::StorageProviderAction,
    templates::TemplatesAction, tokens::TokenAction, users::UsersAction,
};
use error::CliError;
use output::{Output, outln};
use serde_json::{Value, json};
use tond_common::{ERROR_CODES, exit};

const AFTER_HELP: &str = "\
Automation:
  --json prints machine-readable JSON on stdout; errors go to stderr as one line
  {\"error\":{\"code\",\"message\",\"hint\",\"field\"},\"exit_code\":N}.
  Destructive commands need --yes when stdin is not a terminal.

Exit codes:
  0 ok  1 error  2 usage  3 not found  4 denied  5 conflict
  6 build/deploy failed  7 approval required  8 server unavailable
  Details and the error codes behind each: tond help exit-codes";

#[derive(Parser)]
#[command(
    name = "tond",
    about = "Tond CLI - Deploy apps with ease",
    version,
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
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

    /// Print machine-readable JSON on stdout (errors as one JSON line on stderr)
    #[arg(long, global = true)]
    json: bool,

    /// Idempotency-Key sent with mutating requests (default: a new UUID per request).
    /// Repeating a command with the same key returns the first result instead
    /// of acting twice.
    #[arg(long, global = true, env = "TOND_IDEMPOTENCY_KEY")]
    idempotency_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage apps
    #[command(visible_alias = "app")]
    Apps {
        #[command(subcommand)]
        action: AppsAction,
    },

    /// Browse the one-click template catalog
    #[command(visible_alias = "template")]
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },

    /// Manage projects (app groups)
    #[command(visible_alias = "project")]
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
    #[command(visible_alias = "snapshots")]
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },

    /// Manage API tokens (scoped, expiring credentials for automation)
    #[command(visible_alias = "tokens")]
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

    /// Print a shell completion script, e.g. `tond completions bash > /etc/bash_completion.d/tond`
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Print help for a command (`tond help apps deploy`) or the exit-code table
    /// (`tond help exit-codes`)
    Help {
        /// Command path, or `exit-codes`
        topic: Vec<String>,
    },
}

/// The exit-code table with the error codes behind each exit code.
fn exit_codes_json() -> Value {
    let entries: Vec<Value> = exit::TABLE
        .iter()
        .map(|(code, name, meaning)| {
            let api: Vec<&str> = ERROR_CODES
                .iter()
                .filter(|c| c.exit == *code)
                .map(|c| c.code)
                .collect();
            let local: Vec<&str> = error::local_code::TABLE
                .iter()
                .filter(|(_, exit, _)| exit == code)
                .map(|(c, _, _)| *c)
                .collect();
            json!({
                "exit_code": code,
                "name": name,
                "meaning": meaning,
                "error_codes": api,
                "local_codes": local,
            })
        })
        .collect();
    let local: Vec<Value> = error::local_code::TABLE
        .iter()
        .map(|(code, exit, description)| {
            json!({ "code": code, "exit_code": exit, "description": description })
        })
        .collect();
    json!({ "exit_codes": entries, "local_codes": local })
}

fn print_exit_codes(out: &Output) -> Result<()> {
    out.json(&exit_codes_json())?;
    outln!(out, "Exit codes:");
    for (code, name, meaning) in exit::TABLE {
        outln!(out, "  {code}  {name:<18} {meaning}");
        let codes: Vec<&str> = ERROR_CODES
            .iter()
            .filter(|c| c.exit == *code)
            .map(|c| c.code)
            .chain(
                error::local_code::TABLE
                    .iter()
                    .filter(|(_, exit, _)| exit == code)
                    .map(|(c, _, _)| *c),
            )
            .collect();
        if !codes.is_empty() {
            outln!(out, "       {}", codes.join(", "));
        }
    }
    outln!(out);
    outln!(
        out,
        "Codes the CLI reports itself (not sent by the server):"
    );
    for (code, exit, description) in error::local_code::TABLE {
        outln!(out, "  {code:<24} exit {exit}  {description}");
    }
    outln!(out);
    outln!(
        out,
        "With --json, a failure prints one line on stderr:\n  \
         {{\"error\":{{\"code\":…,\"message\":…,\"hint\":…,\"field\":…}},\"exit_code\":N}}\n\
         `code` is null when the server sent none (e.g. a proxy error page)."
    );
    Ok(())
}

/// `tond help [topic…]`.
fn print_help(out: &Output, topic: &[String]) -> Result<()> {
    if topic.len() == 1 && topic[0] == "exit-codes" {
        return print_exit_codes(out);
    }
    let mut cmd = Cli::command();
    cmd.build();
    let mut current = &mut cmd;
    for name in topic {
        current = current.find_subcommand_mut(name).ok_or_else(|| {
            CliError::usage(format!("No help topic '{}'", topic.join(" ")))
                .with_hint("Run `tond help` for the list of commands, or `tond help exit-codes`.")
        })?;
    }
    let text = current.render_long_help().to_string();
    out.json(&json!({ "topic": topic.join(" "), "help": text }))?;
    outln!(out, "{}", text.trim_end());
    Ok(())
}

/// The completion script for `shell`.
fn completion_script(shell: clap_complete::Shell) -> String {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "tond", &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

async fn run(cli: Cli, out: &Output) -> Result<()> {
    // Connect lazily: `scanner`, `update`, `help` and `completions` don't use
    // the API, so they must work without a reachable server.
    let idempotency_key = cli.idempotency_key.clone();
    let connect = || -> Result<TondClient> {
        let client = match cli.server.as_deref() {
            Some(url) => TondClient::new(url, cli.token.as_deref()),
            None => TondClient::local(&cli.socket)?,
        };
        Ok(client.with_idempotency_key(idempotency_key.clone()))
    };

    match cli.command {
        Commands::Apps { action } => commands::apps::handle(&connect()?, out, action).await,
        Commands::Templates { action } => {
            commands::templates::handle(&connect()?, out, action).await
        }
        Commands::Projects { action } => commands::projects::handle(&connect()?, out, action).await,
        Commands::Db { action } => commands::db::handle(&connect()?, out, action).await,
        Commands::Cron { action } => commands::cron::handle(&connect()?, out, action).await,
        Commands::Integrations { action } => {
            commands::integrations::handle(&connect()?, out, action).await
        }
        Commands::Scanner { action } => commands::scanner::handle(out, action).await,
        Commands::Update {
            version,
            dry_run,
            skip_backup,
            force,
        } => commands::update::handle(out, version, dry_run, skip_backup, force),
        Commands::Settings { action } => commands::settings::handle(&connect()?, out, action).await,
        Commands::StorageProvider { action } => {
            commands::storage_providers::handle(&connect()?, out, action).await
        }
        Commands::Snapshot { action } => {
            commands::snapshots::handle(&connect()?, out, action).await
        }
        Commands::Token { action } => commands::tokens::handle(&connect()?, out, action).await,
        Commands::Audit { action } => commands::audit::handle(&connect()?, out, action).await,
        Commands::Users { action } => commands::users::handle(&connect()?, out, action).await,
        Commands::Passwd => commands::settings::handle_passwd(&connect()?, out).await,
        Commands::SetupLink => {
            let data_dir = cli
                .socket
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/opt/tond/data"));
            commands::setup::handle_setup_link(connect(), out, &data_dir).await
        }
        Commands::Completions { shell } => {
            let script = completion_script(shell);
            if out.is_json() {
                out.json(&json!({ "shell": shell.to_string(), "script": script }))
            } else {
                out.raw(&script);
                Ok(())
            }
        }
        Commands::Help { topic } => print_help(out, &topic),
    }
}

/// Report a clap parse error; with --json anywhere on the command line, as a
/// JSON error line. Help and --version keep clap's own output.
fn report_parse_error(err: &clap::Error, json_requested: bool) -> ExitCode {
    let code = err.exit_code();
    let is_error = code != 0;
    if json_requested && is_error {
        let text = err.render().to_string();
        let message = text
            .lines()
            .next()
            .unwrap_or("invalid arguments")
            .trim_start_matches("error: ")
            .to_string();
        let e = CliError::usage(message).with_hint("Run `tond help` for usage.");
        eprintln!(
            "{}",
            json!({ "error": e.to_json(), "exit_code": exit::USAGE })
        );
        return ExitCode::from(exit::USAGE);
    }
    let _ = err.print();
    ExitCode::from(u8::try_from(code).unwrap_or(exit::USAGE))
}

#[tokio::main]
async fn main() -> ExitCode {
    // Behave like other Unix tools when the reader goes away (`tond apps logs
    // | head`, `| grep -q`): exit quietly on SIGPIPE instead of panicking on
    // the failed write. Rust ignores SIGPIPE by default.
    // SAFETY: called first thing in main, before any other thread starts
    // writing; restoring the default disposition is always sound.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    tond_common::install_crypto_provider();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let json_requested = std::env::args().any(|a| a == "--json");
            return report_parse_error(&err, json_requested);
        }
    };

    let json = cli.json;
    let out = Output::stdio(json);
    match run(cli, &out).await {
        Ok(()) => {
            out.finish();
            ExitCode::SUCCESS
        }
        Err(err) => ExitCode::from(error::report(&err, json)),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        http::StatusCode,
        routing::{delete, get, post},
    };
    use clap::Parser;
    use commands::apps::WaitArgs;

    use super::*;
    use crate::client::tests::{app_response_json, spawn_test_server};

    #[test]
    fn compose_service_commands_parse_and_refuse_unknown_actions() {
        let cli = Cli::try_parse_from(["tond", "apps", "service", "shop", "web", "restart"])
            .unwrap_or_else(|err| panic!("apps service should parse: {err}"));
        assert!(matches!(
            cli.command,
            Commands::Apps {
                action: AppsAction::Service { ref app, ref service, ref action }
            } if app == "shop" && service == "web" && action == "restart"
        ));
        assert!(Cli::try_parse_from(["tond", "apps", "service", "shop", "web", "nuke"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["tond", "apps", "services", "shop"])
                .unwrap_or_else(|err| panic!("apps services should parse: {err}"))
                .command,
            Commands::Apps {
                action: AppsAction::Services { .. }
            }
        ));
    }

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
        assert!(!cli.json, "human output is the default");
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

    fn deploy_wait(args: &[&str]) -> Result<WaitArgs, clap::Error> {
        let mut argv = vec!["tond", "apps", "deploy", "web"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).map(|cli| match cli.command {
            Commands::Apps {
                action: AppsAction::Deploy { wait, .. },
            } => wait,
            _ => panic!("expected Apps::Deploy"),
        })
    }

    #[test]
    fn deploy_waits_by_default_and_no_wait_or_no_follow_opt_out() {
        let default = deploy_wait(&[]).unwrap_or_else(|e| panic!("{e}"));
        assert!(default.should_wait());
        let wait = deploy_wait(&["--wait"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(wait.should_wait());
        let no_wait = deploy_wait(&["--no-wait"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(!no_wait.should_wait());
        let no_follow = deploy_wait(&["--no-follow"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(!no_follow.should_wait(), "--no-follow stays an alias");
        let both = deploy_wait(&["--wait", "--no-wait"]);
        assert_eq!(
            both.err().map(|e| e.kind()),
            Some(clap::error::ErrorKind::ArgumentConflict)
        );
        assert!(
            Cli::try_parse_from(["tond", "apps", "push", "web", "--no-wait"]).is_ok(),
            "push takes the same flags"
        );
    }

    #[test]
    fn global_flags_parse_anywhere() {
        let cli = Cli::try_parse_from([
            "tond",
            "apps",
            "delete",
            "web",
            "--json",
            "-y",
            "--idempotency-key",
            "k1",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(cli.json);
        assert_eq!(cli.idempotency_key.as_deref(), Some("k1"));
        assert!(Cli::try_parse_from(["tond", "token", "delete", "ci", "--yes"]).is_ok());
        assert!(Cli::try_parse_from(["tond", "apps", "show", "web"]).is_ok());
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    async fn run_capture(args: &[&str], stdin_tty: bool) -> (Result<()>, String, String) {
        let cli = Cli::try_parse_from(args).unwrap_or_else(|e| panic!("parse {args:?}: {e}"));
        let json = cli.json;
        let (out, stdout, stderr) = Output::capture(json, stdin_tty, "");
        let result = run(cli, &out).await;
        if result.is_ok() {
            out.finish();
        }
        (result, stdout.text(), stderr.text())
    }

    fn parse_json(text: &str) -> Value {
        serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {text:?}"))
    }

    /// The JSON the typed model produces for the app fixture.
    fn typed_app() -> Value {
        let app: tond_common::AppResponse =
            serde_json::from_value(app_response_json()).unwrap_or_else(|e| panic!("fixture: {e}"));
        serde_json::to_value(app).unwrap_or_default()
    }

    fn token_json() -> Value {
        json!({
            "id": "tok-1",
            "name": "ci",
            "prefix": "tond_pat_ab12",
            "kind": "ci",
            "scopes": ["apps:read"],
            "app_ids": [],
            "project_ids": [],
            "allowed_ips": [],
            "owner_id": null,
            "owner_label": "local-socket",
            "expires_at": "2026-10-26T00:00:00Z",
            "last_used_at": null,
            "last_used_ip": null,
            "revoked_at": null,
            "created_at": "2026-09-26T00:00:00Z"
        })
    }

    async fn api_server() -> String {
        let router = Router::new()
            .route(
                "/api/v1/apps",
                get(|| async {
                    axum::Json(json!({"success": true, "data": [app_response_json()]}))
                }),
            )
            .route(
                "/api/v1/apps/demo",
                get(|| async { axum::Json(json!({"success": true, "data": app_response_json()})) })
                    .delete(|| async {
                        axum::Json(json!({"success": true, "data": {"message": "App deleted"}}))
                    }),
            )
            .route(
                "/api/v1/apps/missing",
                get(|| async {
                    (
                        StatusCode::NOT_FOUND,
                        axum::Json(json!({"success": false, "data": null, "error": {
                            "code": "APP_NOT_FOUND",
                            "message": "App not found: missing",
                            "hint": "List apps with: tond apps list"
                        }})),
                    )
                }),
            )
            .route(
                "/api/v1/apps/demo/deploy",
                post(|| async {
                    (
                        StatusCode::ACCEPTED,
                        axum::Json(json!({"success": true, "data": {
                            "message": "Deployment queued", "app_id": "app-1",
                            "deployment_id": "dep-1", "status": "queued",
                            "queue_position": 1, "superseded": false
                        }})),
                    )
                }),
            )
            .route(
                "/api/v1/tokens",
                post(|| async {
                    axum::Json(json!({"success": true, "data": {
                        "token": token_json(), "secret": "tond_pat_ab12_secret"
                    }}))
                }),
            )
            .route(
                "/api/v1/cron/nightly",
                delete(|| async { axum::Json(json!({"success": true, "data": null})) }),
            );
        let addr = spawn_test_server(router).await;
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn json_apps_list_and_info_print_the_typed_models() {
        let server = api_server().await;
        let (res, stdout, stderr) = run_capture(
            &["tond", "--server", &server, "--json", "apps", "list"],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(parse_json(&stdout), json!([typed_app()]));
        assert_eq!(stderr, "");

        let (res, stdout, _) = run_capture(
            &[
                "tond", "--server", &server, "--json", "apps", "info", "demo",
            ],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(parse_json(&stdout), typed_app());
    }

    #[tokio::test]
    async fn token_create_prints_the_secret_alone_or_the_whole_object() {
        let server = api_server().await;
        let args = [
            "tond",
            "--server",
            &server,
            "token",
            "create",
            "--name",
            "ci",
            "--kind",
            "ci",
            "--preset",
            "deployer",
            "--expires",
            "30d",
        ];
        let (res, stdout, stderr) = run_capture(&args, false).await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(
            stdout, "tond_pat_ab12_secret\n",
            "human mode: the secret alone"
        );
        assert!(stderr.contains("Store it now"));

        let mut json_args = args.to_vec();
        json_args.insert(3, "--json");
        let (res, stdout, stderr) = run_capture(&json_args, false).await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(
            parse_json(&stdout),
            json!({"token": token_json(), "secret": "tond_pat_ab12_secret"})
        );
        assert_eq!(stderr, "", "no decoration in JSON mode");
    }

    #[tokio::test]
    async fn delete_with_yes_prints_the_standard_result() {
        let server = api_server().await;
        let (res, stdout, _) = run_capture(
            &[
                "tond", "--server", &server, "--json", "apps", "delete", "demo", "--yes",
            ],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(
            parse_json(&stdout),
            json!({"ok": true, "action": "delete", "resource": "app", "id": "demo",
                   "data": {"message": "App deleted"}})
        );

        let (res, stdout, _) = run_capture(
            &[
                "tond", "--server", &server, "--json", "cron", "delete", "nightly", "-y",
            ],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(
            parse_json(&stdout),
            json!({"ok": true, "action": "delete", "resource": "cron_job", "id": "nightly", "data": null})
        );
    }

    #[tokio::test]
    async fn destructive_commands_refuse_without_yes_and_a_terminal() {
        let server = api_server().await;
        for args in [
            vec!["apps", "delete", "demo"],
            vec!["apps", "rollback", "demo"],
            vec!["apps", "volume", "remove", "demo", "/data"],
            vec!["apps", "previews", "delete", "demo", "p1"],
            vec!["projects", "delete", "p"],
            vec!["projects", "preview", "delete", "p", "pp1"],
            vec!["db", "delete", "d"],
            vec!["cron", "delete", "nightly"],
            vec!["integrations", "delete", "i1"],
            vec!["storage-provider", "delete", "s1"],
            vec!["snapshot", "delete", "s1"],
            vec!["snapshot", "restore", "s1"],
            vec!["snapshot", "rollback", "s1", "--app", "demo"],
        ] {
            let mut argv = vec!["tond", "--server", &server];
            argv.extend(args.iter().copied());
            let (res, stdout, _) = run_capture(&argv, false).await;
            let err = res.err().unwrap_or_else(|| panic!("{args:?} must refuse"));
            let e = error::describe(&err);
            assert_eq!(e.exit, 2, "{args:?}");
            assert_eq!(e.code.as_deref(), Some("USAGE"));
            assert!(
                e.message.starts_with("Refusing to ") && e.message.contains("without --yes"),
                "{args:?}: {}",
                e.message
            );
            assert_eq!(stdout, "", "{args:?}: nothing on stdout");
        }
    }

    #[tokio::test]
    async fn api_errors_render_as_json_with_their_exit_code() {
        let server = api_server().await;
        let (res, stdout, _) = run_capture(
            &[
                "tond", "--server", &server, "--json", "apps", "info", "missing",
            ],
            false,
        )
        .await;
        let err = res.err().unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(stdout, "");
        assert_eq!(error::exit_code(&err), 3);
        assert_eq!(
            parse_json(&error::render(&err, true)),
            json!({
                "error": {
                    "code": "APP_NOT_FOUND",
                    "message": "App not found: missing",
                    "hint": "List apps with: tond apps list",
                    "field": null
                },
                "exit_code": 3
            })
        );
        assert_eq!(
            error::render(&err, false),
            "Error: App not found: missing\nHint: List apps with: tond apps list"
        );
    }

    #[tokio::test]
    async fn deploy_no_wait_prints_the_queued_deployment() {
        let server = api_server().await;
        let (res, stdout, _) = run_capture(
            &[
                "tond",
                "--server",
                &server,
                "--json",
                "apps",
                "deploy",
                "demo",
                "--no-wait",
            ],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(
            parse_json(&stdout),
            json!({
                "message": "Deployment queued", "app_id": "app-1", "app": "demo",
                "deployment_id": "dep-1", "status": "queued",
                "queue_position": 1, "superseded": false
            })
        );

        let (res, stdout, _) = run_capture(
            &[
                "tond",
                "--server",
                &server,
                "apps",
                "deploy",
                "demo",
                "--no-follow",
            ],
            false,
        )
        .await;
        assert!(res.is_ok(), "{res:?}");
        assert!(
            stdout.contains("dep-1") && stdout.contains("queued"),
            "{stdout}"
        );
    }

    #[tokio::test]
    async fn deploy_wait_streams_ndjson_and_exits_6_on_a_failed_build() {
        use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};

        let running = json!({"status": "running", "phases": [
            {"phase": "clone", "status": "succeeded", "started_at": "2026-09-26T10:00:00Z", "finished_at": "2026-09-26T10:00:02Z"},
            {"phase": "build", "status": "running", "started_at": "2026-09-26T10:00:02Z"}
        ]});
        let failed = json!({"status": "failed",
            "phases": [
                {"phase": "clone", "status": "succeeded", "started_at": "2026-09-26T10:00:00Z", "finished_at": "2026-09-26T10:00:02Z"},
                {"phase": "build", "status": "failed", "started_at": "2026-09-26T10:00:02Z", "finished_at": "2026-09-26T10:00:09Z"}
            ],
            "failure": {"phase": "build", "code": "build_failed", "title": "The build failed (exit code 1)", "hint": "Check the build log."}
        });
        let frames = vec![
            json!({"type": "snapshot", "deployment": running, "logs": [{"message": "Step 1/3 : FROM node"}]}),
            json!({"type": "log", "message": "npm ERR! missing script: build"}),
            json!({"type": "deployment", "deployment": failed}),
        ];
        let router = Router::new()
            .route(
                "/api/v1/apps/demo/deploy",
                post(|| async {
                    axum::Json(json!({"success": true, "data": {
                        "deployment_id": "dep-1", "status": "queued", "queue_position": 1
                    }}))
                }),
            )
            .route(
                "/api/v1/apps/demo/deployments/dep-1/stream",
                get(move |ws: WebSocketUpgrade| {
                    let frames = frames.clone();
                    async move {
                        ws.on_upgrade(move |mut socket| async move {
                            for frame in frames {
                                let _ =
                                    socket.send(WsMessage::Text(frame.to_string().into())).await;
                            }
                            let _ = socket.send(WsMessage::Close(None)).await;
                        })
                    }
                }),
            );
        let server = format!("http://{}", spawn_test_server(router).await);

        let (res, stdout, _) = run_capture(
            &[
                "tond", "--server", &server, "--json", "apps", "deploy", "demo", "--wait",
            ],
            false,
        )
        .await;
        let err = res
            .err()
            .unwrap_or_else(|| panic!("a failed build must fail the command"));
        let e = error::describe(&err);
        assert_eq!((e.exit, e.code.as_deref()), (6, Some("BUILD_FAILED")));

        let events: Vec<Value> = stdout.lines().map(parse_json).collect();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| e["event"].as_str().unwrap_or("?"))
            .collect();
        assert_eq!(
            kinds,
            vec!["queued", "log", "phase", "phase", "log", "phase", "result"],
            "{stdout}"
        );
        assert_eq!(events[1]["message"], "Step 1/3 : FROM node");
        assert_eq!(
            events[3],
            json!({"event": "phase", "phase": "build", "status": "running",
            "label": null, "started_at": "2026-09-26T10:00:02Z", "finished_at": null})
        );
        let result = &events[6];
        assert_eq!(result["status"], "failed");
        assert_eq!(result["exit_code"], 6);
        assert_eq!(result["error"]["code"], "BUILD_FAILED");
        assert_eq!(result["failure"]["code"], "build_failed");
    }

    #[tokio::test]
    async fn unreachable_server_exits_8() {
        let (res, _, _) = run_capture(
            &["tond", "--socket", "/nonexistent/tond.sock", "apps", "list"],
            false,
        )
        .await;
        let err = res.err().unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(error::exit_code(&err), 8);
    }

    #[tokio::test]
    async fn help_exit_codes_json_lists_every_exit_code() {
        let (res, stdout, _) = run_capture(&["tond", "help", "exit-codes", "--json"], false).await;
        assert!(res.is_ok(), "{res:?}");
        let doc = parse_json(&stdout);
        let entries = doc["exit_codes"]
            .as_array()
            .unwrap_or_else(|| panic!("exit_codes array: {doc}"));
        assert_eq!(entries.len(), exit::TABLE.len());
        assert_eq!(entries[0]["exit_code"], 0);
        assert_eq!(entries[0]["name"], "ok");
        let not_found = &entries[3];
        assert_eq!(not_found["exit_code"], 3);
        assert!(
            not_found["error_codes"]
                .as_array()
                .is_some_and(|c| c.contains(&json!("APP_NOT_FOUND")))
        );
        let unavailable = &entries[8];
        assert!(
            unavailable["local_codes"]
                .as_array()
                .is_some_and(|c| c.contains(&json!("SERVER_UNREACHABLE")))
        );
        assert!(doc["local_codes"].as_array().is_some_and(|c| !c.is_empty()));

        let (res, stdout, _) = run_capture(&["tond", "help", "exit-codes"], false).await;
        assert!(res.is_ok());
        assert!(stdout.contains("APPROVAL_REQUIRED"));
    }

    #[tokio::test]
    async fn help_prints_command_help_and_rejects_unknown_topics() {
        let (res, stdout, _) = run_capture(&["tond", "help", "apps", "deploy"], false).await;
        assert!(res.is_ok(), "{res:?}");
        assert!(stdout.contains("--no-wait"), "{stdout}");
        let (res, stdout, _) = run_capture(&["tond", "help"], false).await;
        assert!(res.is_ok());
        assert!(
            stdout.contains("Exit codes"),
            "top-level help mentions exit codes"
        );
        let (res, _, _) = run_capture(&["tond", "help", "nope"], false).await;
        assert_eq!(res.err().map(|e| error::exit_code(&e)), Some(2));
    }

    #[tokio::test]
    async fn completions_for_bash_are_generated() {
        let (res, stdout, _) = run_capture(&["tond", "completions", "bash"], false).await;
        assert!(res.is_ok(), "{res:?}");
        assert!(stdout.contains("tond") && stdout.len() > 1000);
        assert!(completion_script(clap_complete::Shell::Zsh).contains("#compdef tond"));
    }

    #[test]
    fn parse_errors_exit_2() {
        let err = Cli::try_parse_from(["tond", "apps", "frobnicate"])
            .err()
            .unwrap_or_else(|| panic!("expected a parse error"));
        assert_eq!(err.exit_code(), 2);
    }
}
