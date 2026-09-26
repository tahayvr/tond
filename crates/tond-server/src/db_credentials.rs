// db_credentials.rs — change a managed database's password inside the running engine
//
// Database images only read their *_PASSWORD env vars when they initialise an
// empty data directory. Once data exists, changing the env and redeploying does
// nothing, so the password Tond stores and the real one drift apart. Instead we
// run the engine's own password change (ALTER ROLE / ALTER USER /
// changeUserPassword) inside the running container with `docker exec`,
// authenticated with the current credentials, and only persist the new value
// once that succeeded.
//
// Secrets never pass through a shell string: they travel in the exec's env and
// are referenced as "$VAR" by fixed scripts, or are hex-encoded into SQL that
// MySQL decodes server-side. Command output is redacted before it is surfaced.

use std::time::Duration;

use bollard::{Docker, exec::CreateExecOptions, exec::StartExecResults};
use futures::StreamExt;
use tond_common::ManagedDatabase;

/// Which stored credential to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialField {
    /// The application user's password (`db_password`)
    AppPassword,
    /// The MySQL/MariaDB root password (`root_password`)
    RootPassword,
}

impl CredentialField {
    pub fn label(self) -> &'static str {
        match self {
            CredentialField::AppPassword => "password",
            CredentialField::RootPassword => "root password",
        }
    }
}

/// One `docker exec` invocation: argv (no shell parsing of secrets) plus env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecStep {
    pub cmd: Vec<String>,
    pub env: Vec<String>,
}

// psql reads the role and new password from psql variables, which `:"x"` and
// `:'x'` quote as an identifier and a literal. The heredoc is quoted, so the
// shell expands nothing inside it.
const POSTGRES_SCRIPT: &str = r#"psql -v ON_ERROR_STOP=1 -X -q -U "$TOND_DB_USER" -d "$TOND_DB_NAME" -v role="$TOND_DB_USER" -v newpw="$TOND_NEW_PASSWORD" <<'SQL'
ALTER ROLE :"role" WITH PASSWORD :'newpw';
SQL"#;

// MariaDB 11 images ship only the `mariadb` client; MySQL images only `mysql`.
// The SQL arrives via env and is piped on stdin (printf is a shell builtin, so
// it never shows up in a process list).
const MYSQL_SCRIPT: &str = r#"if command -v mariadb >/dev/null 2>&1; then c=mariadb; else c=mysql; fi; printf '%s\n' "$TOND_SQL" | "$c" -uroot"#;

// mongosh reads everything from process.env; no shell involved.
const MONGO_SCRIPT: &str = "const admin = db.getSiblingDB('admin'); \
admin.auth(process.env.TOND_DB_USER, process.env.TOND_CURRENT_PASSWORD); \
admin.changeUserPassword(process.env.TOND_DB_USER, process.env.TOND_NEW_PASSWORD);";

fn hex(value: &str) -> String {
    hex::encode(value.as_bytes())
}

/// SQL that runs `ALTER USER <user>@<host> IDENTIFIED BY <password>` through a
/// prepared statement. User and password are hex literals decoded by the server
/// and quoted with QUOTE(), so no input can break out of the statement.
fn mysql_alter_user_sql(user: &str, hosts: &[&str], password: &str) -> String {
    let mut sql = format!(
        "SET @tond_user = CONVERT(X'{}' USING utf8mb4);\nSET @tond_pw = CONVERT(X'{}' USING utf8mb4);\n",
        hex(user),
        hex(password)
    );
    for host in hosts {
        sql.push_str(&format!(
            "SET @tond_sql = CONCAT('ALTER USER IF EXISTS ', QUOTE(@tond_user), '@', QUOTE(CONVERT(X'{}' USING utf8mb4)), ' IDENTIFIED BY ', QUOTE(@tond_pw));\nPREPARE tond_stmt FROM @tond_sql;\nEXECUTE tond_stmt;\nDEALLOCATE PREPARE tond_stmt;\n",
            hex(host)
        ));
    }
    sql
}

/// Build the exec that changes `field` to `new_password` for `db`, or explain
/// why the engine can't do it.
pub fn password_change_step(
    db: &ManagedDatabase,
    field: CredentialField,
    new_password: &str,
) -> Result<ExecStep, String> {
    let user = db.db_user.clone().unwrap_or_default();
    let current = db.db_password.clone().unwrap_or_default();
    match (db.db_type.as_str(), field) {
        ("postgres", CredentialField::AppPassword) => Ok(ExecStep {
            cmd: vec!["sh".into(), "-c".into(), POSTGRES_SCRIPT.into()],
            env: vec![
                format!("TOND_DB_USER={user}"),
                format!("TOND_DB_NAME={}", db.db_name.clone().unwrap_or_else(|| user.clone())),
                format!("TOND_NEW_PASSWORD={new_password}"),
                format!("PGPASSWORD={current}"),
            ],
        }),
        ("mysql" | "mariadb", _) => {
            // The root password falls back to the app password, matching build_env().
            let root = db.root_password.clone().unwrap_or_else(|| current.clone());
            let sql = match field {
                CredentialField::AppPassword => mysql_alter_user_sql(&user, &["%"], new_password),
                CredentialField::RootPassword => {
                    mysql_alter_user_sql("root", &["localhost", "%"], new_password)
                }
            };
            Ok(ExecStep {
                cmd: vec!["sh".into(), "-c".into(), MYSQL_SCRIPT.into()],
                env: vec![format!("MYSQL_PWD={root}"), format!("TOND_SQL={sql}")],
            })
        }
        ("mongodb", CredentialField::AppPassword) => Ok(ExecStep {
            cmd: vec![
                "mongosh".into(),
                "--quiet".into(),
                "--norc".into(),
                "--eval".into(),
                MONGO_SCRIPT.into(),
            ],
            env: vec![
                format!("TOND_DB_USER={user}"),
                format!("TOND_CURRENT_PASSWORD={current}"),
                format!("TOND_NEW_PASSWORD={new_password}"),
            ],
        }),
        ("redis" | "keydb", _) => Err(
            "Redis and KeyDB passwords can't be changed in place yet. The stored password was not changed."
                .to_string(),
        ),
        (engine, CredentialField::RootPassword) => Err(format!(
            "{engine} databases don't have a separate root password."
        )),
        (engine, _) => Err(format!(
            "Changing the password of a {engine} database isn't supported."
        )),
    }
}

/// Replace every secret (and its hex form, as used in MySQL SQL) with `****`.
pub fn redact(output: &str, secrets: &[&str]) -> String {
    let mut redacted = output.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        redacted = redacted.replace(secret, "****");
        redacted = redacted.replace(&hex(secret), "****");
        redacted = redacted.replace(&hex(secret).to_uppercase(), "****");
    }
    redacted
}

const EXEC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 4_000;

/// Run `step` in the container and fail with its (redacted) output if it exits
/// non-zero or doesn't finish within 30 s.
pub async fn run_step(
    docker: &Docker,
    container_id: &str,
    step: &ExecStep,
    secrets: &[&str],
) -> anyhow::Result<()> {
    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(step.cmd.clone()),
                env: Some(step.env.clone()),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    let run = async {
        let mut output = String::new();
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = docker.start_exec(&exec.id, None).await?
        {
            while let Some(chunk) = stream.next().await {
                if output.len() < MAX_OUTPUT {
                    output.push_str(&chunk?.to_string());
                }
            }
        }
        anyhow::Ok(output)
    };
    let output = tokio::time::timeout(EXEC_TIMEOUT, run)
        .await
        .map_err(|_| anyhow::anyhow!("the database did not respond within 30 seconds"))??;

    let exit_code = docker.inspect_exec(&exec.id).await?.exit_code.unwrap_or(-1);
    if exit_code != 0 {
        let detail = redact(output.trim(), secrets);
        let detail: String = detail.chars().take(500).collect();
        anyhow::bail!(
            "the database rejected the change (exit code {exit_code}){}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(())
}

/// Whether the container is up, so an exec can run in it.
pub async fn container_running(docker: &Docker, container_id: &str) -> bool {
    docker
        .inspect_container(container_id, None)
        .await
        .ok()
        .and_then(|info| info.state)
        .and_then(|state| state.running)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(db_type: &str) -> ManagedDatabase {
        let mut db = ManagedDatabase::new("shop".to_string(), db_type.to_string(), None);
        db.db_password = Some("old-pass".to_string());
        db
    }

    fn env_value<'a>(step: &'a ExecStep, key: &str) -> Option<&'a str> {
        step.env
            .iter()
            .find_map(|e| e.strip_prefix(&format!("{key}=")))
    }

    const NASTY: &str = "p'a\"ss$(rm -rf /)`x`;\\ \n--";

    #[test]
    fn postgres_passes_secrets_only_through_env() {
        let step = password_change_step(&db("postgres"), CredentialField::AppPassword, NASTY)
            .unwrap_or_else(|e| panic!("postgres should be supported: {e}"));

        assert_eq!(step.cmd[..2], ["sh".to_string(), "-c".to_string()]);
        assert_eq!(
            step.cmd[2], POSTGRES_SCRIPT,
            "the script must be a constant"
        );
        assert!(
            !step
                .cmd
                .iter()
                .any(|arg| arg.contains("old-pass") || arg.contains(NASTY))
        );
        assert_eq!(env_value(&step, "TOND_NEW_PASSWORD"), Some(NASTY));
        assert_eq!(env_value(&step, "PGPASSWORD"), Some("old-pass"));
        assert_eq!(env_value(&step, "TOND_DB_USER"), Some("tond_shop"));
    }

    #[test]
    fn mysql_hex_encodes_the_password_into_sql_and_authenticates_as_root() {
        let mut database = db("mysql");
        database.root_password = Some("root-pass".to_string());

        let step = password_change_step(&database, CredentialField::AppPassword, NASTY)
            .unwrap_or_else(|e| panic!("mysql should be supported: {e}"));

        assert_eq!(step.cmd[2], MYSQL_SCRIPT);
        assert_eq!(env_value(&step, "MYSQL_PWD"), Some("root-pass"));
        let sql = env_value(&step, "TOND_SQL").unwrap_or_default();
        assert!(
            !sql.contains(NASTY),
            "the raw password must not appear in SQL"
        );
        assert!(sql.contains(&hex(NASTY)));
        assert!(sql.contains(&hex("tond_shop")));
        assert!(
            sql.chars().all(|c| c != '`' && c != '$'),
            "only hex literals and fixed SQL may appear: {sql}"
        );
    }

    #[test]
    fn mysql_root_password_changes_both_root_accounts() {
        let mut database = db("mariadb");
        database.root_password = None;
        let step = password_change_step(&database, CredentialField::RootPassword, "new-root")
            .unwrap_or_else(|e| panic!("mariadb root should be supported: {e}"));

        let sql = env_value(&step, "TOND_SQL").unwrap_or_default();
        assert!(sql.contains(&hex("root")));
        assert!(sql.contains(&hex("localhost")));
        assert!(sql.contains(&hex("%")));
        assert_eq!(
            env_value(&step, "MYSQL_PWD"),
            Some("old-pass"),
            "without a stored root password, root mirrors the app password"
        );
    }

    #[test]
    fn mongo_runs_mongosh_without_a_shell() {
        let step = password_change_step(&db("mongodb"), CredentialField::AppPassword, NASTY)
            .unwrap_or_else(|e| panic!("mongodb should be supported: {e}"));

        assert_eq!(step.cmd[0], "mongosh");
        assert!(!step.cmd.iter().any(|arg| arg.contains(NASTY)));
        assert_eq!(env_value(&step, "TOND_CURRENT_PASSWORD"), Some("old-pass"));
    }

    #[test]
    fn unsupported_changes_are_refused_with_a_reason() {
        assert!(password_change_step(&db("redis"), CredentialField::AppPassword, "x").is_err());
        assert!(password_change_step(&db("keydb"), CredentialField::AppPassword, "x").is_err());
        assert!(password_change_step(&db("postgres"), CredentialField::RootPassword, "x").is_err());
    }

    #[test]
    fn redact_hides_plain_and_hex_secrets() {
        let out = format!(
            "LINE 1: ALTER ROLE x PASSWORD 'hunter2'; near X'{}'",
            hex("hunter2")
        );
        assert_eq!(
            redact(&out, &["hunter2", ""]),
            "LINE 1: ALTER ROLE x PASSWORD '****'; near X'****'"
        );
    }
}
