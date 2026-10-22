// db_exec.rs — run commands inside a managed database container
//
// Dumps, imports, readiness probes and extension helpers all go through
// `run`: one `docker exec` with argv (never a shell string holding a secret),
// secrets in its env, bounded output and a timeout, and the exit code
// checked. Errors carry the command's output with secrets redacted.

use std::time::Duration;

use anyhow::{Context, Result};
use bollard::{Docker, exec::CreateExecOptions, exec::StartExecResults};
use futures::StreamExt;
use tond_common::ManagedDatabase;

/// Output kept from a command (the tail matters less than the first error).
const MAX_OUTPUT: usize = 64 * 1024;

/// One command to run in a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub argv: Vec<String>,
    pub env: Vec<String>,
    /// Exec user (a name or "uid:gid"); None = the image's user.
    pub user: Option<String>,
}

impl Cmd {
    pub fn new(argv: &[&str]) -> Self {
        Self {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            user: None,
        }
    }

    pub fn sh(script: &str) -> Self {
        Self::new(&["sh", "-c", script])
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push(format!("{key}={value}"));
        self
    }
}

/// Replace every non-empty secret in `text`.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(s, "••••••••");
    }
    out
}

/// Run `cmd` in `container_id`; returns its output (stdout + stderr). A
/// non-zero exit code is an error that quotes the (redacted) output.
pub async fn run(
    docker: &Docker,
    container_id: &str,
    cmd: &Cmd,
    timeout: Duration,
    secrets: &[&str],
) -> Result<String> {
    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(cmd.argv.clone()),
                env: Some(cmd.env.clone()),
                user: cmd.user.clone(),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .context("the container isn't running")?;

    let collect = async {
        let mut output = String::new();
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = docker.start_exec(&exec.id, None).await?
        {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?.to_string();
                if output.len() < MAX_OUTPUT {
                    output.push_str(&chunk);
                }
            }
        }
        anyhow::Ok(output)
    };
    let output = tokio::time::timeout(timeout, collect)
        .await
        .map_err(|_| anyhow::anyhow!("gave up after {} seconds", timeout.as_secs()))??;

    let exit_code = docker.inspect_exec(&exec.id).await?.exit_code.unwrap_or(-1);
    if exit_code != 0 {
        let detail = redact(output.trim(), secrets);
        let detail: String = detail.chars().take(600).collect();
        anyhow::bail!(
            "{} exited with code {exit_code}{}",
            cmd.argv.first().map(String::as_str).unwrap_or("command"),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(output)
}

/// The secrets of a (decrypted) database, for redaction.
pub fn secrets_of(db: &ManagedDatabase) -> Vec<&str> {
    [db.db_password.as_deref(), db.root_password.as_deref()]
        .into_iter()
        .flatten()
        .collect()
}

/// A command that succeeds once the engine accepts connections over TCP.
/// TCP matters: the images' first-start initialisation runs a temporary
/// server on the local socket only, then restarts it.
pub fn ready_probe(db: &ManagedDatabase) -> Option<Cmd> {
    let user = db.db_user.as_deref().unwrap_or_default();
    let pass = db.db_password.as_deref().unwrap_or_default();
    Some(match db.db_type.as_str() {
        "postgres" => Cmd::new(&[
            "pg_isready",
            "-h",
            "127.0.0.1",
            "-U",
            user,
            "-d",
            db.db_name.as_deref().unwrap_or("postgres"),
        ]),
        "mysql" | "mariadb" => Cmd::sh(
            r#"if command -v mariadb >/dev/null 2>&1; then c=mariadb; else c=mysql; fi; "$c" -h 127.0.0.1 -uroot -e 'SELECT 1' >/dev/null"#,
        )
        .env(
            "MYSQL_PWD",
            db.root_password.as_deref().unwrap_or(pass),
        ),
        "redis" | "keydb" => {
            let cli = if db.db_type == "keydb" {
                "keydb-cli"
            } else {
                "redis-cli"
            };
            Cmd::sh(&format!(
                r#"{cli} -h 127.0.0.1 --user "$TOND_DB_USER" --no-auth-warning PING | grep -qx PONG"#
            ))
            .env("TOND_DB_USER", user)
            .env("REDISCLI_AUTH", pass)
        }
        "mongodb" => Cmd::sh(
            r#"mongosh --quiet --host 127.0.0.1 --authenticationDatabase admin -u "$TOND_DB_USER" -p "$TOND_DB_PASSWORD" --eval 'db.adminCommand({ping: 1}).ok' | grep -qx 1"#,
        )
        .env("TOND_DB_USER", user)
        .env("TOND_DB_PASSWORD", pass),
        _ => return None,
    })
}

/// Wait until the engine answers twice in a row (a first-start restart can
/// fall between two probes), or fail after `timeout`.
pub async fn wait_ready(
    docker: &Docker,
    container_id: &str,
    db: &ManagedDatabase,
    timeout: Duration,
) -> Result<()> {
    let Some(probe) = ready_probe(db) else {
        tokio::time::sleep(Duration::from_secs(3)).await;
        return Ok(());
    };
    let deadline = tokio::time::Instant::now() + timeout;
    let secrets = secrets_of(db);
    let mut streak = 0;
    let mut last_error = String::new();
    while tokio::time::Instant::now() < deadline {
        match run(
            docker,
            container_id,
            &probe,
            Duration::from_secs(10),
            &secrets,
        )
        .await
        {
            Ok(_) => {
                streak += 1;
                if streak >= 2 {
                    return Ok(());
                }
            }
            Err(e) => {
                streak = 0;
                last_error = format!("{e:#}");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    anyhow::bail!(
        "the database didn't become ready within {} seconds{}",
        timeout.as_secs(),
        if last_error.is_empty() {
            String::new()
        } else {
            format!(" ({last_error})")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(db_type: &str) -> ManagedDatabase {
        let mut db = ManagedDatabase::new("shop".to_string(), db_type.to_string(), None);
        db.db_password = Some("s3cret-pass".to_string());
        db.root_password = Some("r00t-pass".to_string());
        db
    }

    #[test]
    fn probes_use_tcp_and_keep_passwords_out_of_argv() {
        for kind in ["postgres", "mysql", "mariadb", "redis", "keydb", "mongodb"] {
            let d = db(kind);
            let probe = ready_probe(&d).unwrap_or_else(|| panic!("{kind} has a probe"));
            let argv = probe.argv.join(" ");
            assert!(argv.contains("127.0.0.1"), "{kind}: {argv}");
            assert!(
                !argv.contains("s3cret-pass") && !argv.contains("r00t-pass"),
                "{kind} must pass secrets through env: {argv}"
            );
        }
        assert!(ready_probe(&db("sqlite")).is_none());
    }

    #[test]
    fn redact_hides_secrets() {
        assert_eq!(
            redact("auth failed for s3cret-pass", &["s3cret-pass", ""]),
            "auth failed for ••••••••"
        );
    }
}
