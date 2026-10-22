// compose_backup.rs — backup hooks of compose services
//
// A database's files copied while it runs are crash-consistent at best. A
// compose service can declare how to dump and load its data with two labels:
//
//   labels:
//     tond.backup.dump: pg_dump -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" --clean --if-exists
//     tond.backup.restore: psql -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" -q -v ON_ERROR_STOP=1
//
// Before a snapshot copies the app's volumes, each dump command runs with
// `sh -c` inside the service's container (docker exec) and its stdout is
// stored in the snapshot as the service's dump. After a restore, the restore
// command runs the same way with that dump on stdin. A dump command may also
// write into the service's own volume instead (`redis-cli SAVE`), so the copy
// that follows is consistent.
//
// Both stream: stdout goes to a file and the file to stdin in fixed-size
// chunks, so memory stays flat however big the database is. The commands run
// in the user's own container, with its environment: they grant nothing the
// compose file didn't already. Label values are checked by
// `tond_common::check_backup_hook` (compose_policy.rs, template validation).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::query_parameters::ListContainersOptionsBuilder;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::compose::{PROJECT_LABEL, SERVICE_LABEL};

pub const DUMP_LABEL: &str = tond_common::BACKUP_DUMP_LABEL;
pub const RESTORE_LABEL: &str = tond_common::BACKUP_RESTORE_LABEL;

/// How long a dump or a restore may run.
const HOOK_TIMEOUT: Duration = Duration::from_secs(6 * 3600);
/// How long a restore is retried while the service isn't ready yet.
const RESTORE_READY_TIMEOUT: Duration = Duration::from_secs(180);
/// Stderr kept for error messages (the tail matters most).
const STDERR_TAIL: usize = 4 * 1024;

/// A service's backup hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupHook {
    pub service: String,
    pub container_id: String,
    pub dump: String,
    pub restore: Option<String>,
}

/// The hook a running container declares in its labels, if any.
fn hook_from_labels(container_id: &str, labels: &HashMap<String, String>) -> Option<BackupHook> {
    let service = labels.get(SERVICE_LABEL)?;
    let dump = labels
        .get(DUMP_LABEL)
        .filter(|d| tond_common::check_backup_hook(DUMP_LABEL, d).is_ok())?;
    let restore = labels
        .get(RESTORE_LABEL)
        .filter(|r| tond_common::check_backup_hook(RESTORE_LABEL, r).is_ok())
        .cloned();
    Some(BackupHook {
        service: service.clone(),
        container_id: container_id.to_string(),
        dump: dump.clone(),
        restore,
    })
}

/// Hooks declared by the running containers of compose app `app_name`,
/// sorted by service (one per service).
pub async fn discover_hooks(docker: &Docker, app_name: &str) -> Result<Vec<BackupHook>> {
    let project = crate::deploy::compose_project_name(app_name);
    let filters = HashMap::from([
        (
            "label".to_string(),
            vec![format!("{PROJECT_LABEL}={project}"), DUMP_LABEL.to_string()],
        ),
        ("status".to_string(), vec!["running".to_string()]),
    ]);
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
        .with_context(|| format!("could not list the containers of {app_name}"))?;
    let mut hooks: Vec<BackupHook> = containers
        .into_iter()
        .filter_map(|c| hook_from_labels(c.id.as_deref()?, &c.labels.unwrap_or_default()))
        .collect();
    hooks.sort_by(|a, b| a.service.cmp(&b.service));
    hooks.dedup_by(|a, b| a.service == b.service);
    Ok(hooks)
}

/// The last `STDERR_TAIL` bytes of stderr, kept while a command runs.
#[derive(Default)]
struct Tail(Vec<u8>);

impl Tail {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
        if self.0.len() > STDERR_TAIL {
            let cut = self.0.len() - STDERR_TAIL;
            self.0.drain(..cut);
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0).trim().to_string()
    }
}

/// What ended a hook command.
fn failure(what: &str, service: &str, exit_code: i64, stderr: &Tail) -> anyhow::Error {
    let detail = stderr.text();
    anyhow::anyhow!(
        "the {what} command of service {service} exited with code {exit_code}{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )
}

async fn create_exec(
    docker: &Docker,
    hook: &BackupHook,
    command: &str,
    stdin: bool,
) -> Result<String> {
    let exec = docker
        .create_exec(
            &hook.container_id,
            CreateExecOptions {
                cmd: Some(vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    command.to_string(),
                ]),
                attach_stdin: Some(stdin),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("service {} isn't running", hook.service))?;
    Ok(exec.id)
}

/// Run the dump command; its stdout goes to `dest` (created or truncated).
/// Returns the bytes written.
pub async fn run_dump(docker: &Docker, hook: &BackupHook, dest: &Path) -> Result<u64> {
    let exec_id = create_exec(docker, hook, &hook.dump, false).await?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("could not create {}", dest.display()))?;
    let mut stderr = Tail::default();
    let collect = async {
        let mut written = 0u64;
        if let StartExecResults::Attached { mut output, .. } =
            docker.start_exec(&exec_id, None).await?
        {
            while let Some(chunk) = output.next().await {
                match chunk? {
                    LogOutput::StdOut { message } => {
                        file.write_all(&message).await?;
                        written += message.len() as u64;
                    }
                    LogOutput::StdErr { message } => stderr.push(&message),
                    _ => {}
                }
            }
        }
        file.flush().await?;
        anyhow::Ok(written)
    };
    let written = tokio::time::timeout(HOOK_TIMEOUT, collect)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the dump of service {} gave up after {} hours",
                hook.service,
                HOOK_TIMEOUT.as_secs() / 3600
            )
        })??;
    let exit_code = docker.inspect_exec(&exec_id).await?.exit_code.unwrap_or(-1);
    if exit_code != 0 {
        return Err(failure("dump", &hook.service, exit_code, &stderr));
    }
    Ok(written)
}

/// One attempt at the restore command with `dump` on stdin. Returns the exit
/// code and the stderr tail.
async fn restore_once(
    docker: &Docker,
    hook: &BackupHook,
    restore: &str,
    dump: &Path,
) -> Result<(i64, Tail)> {
    let exec_id = create_exec(docker, hook, restore, true).await?;
    let mut file = tokio::fs::File::open(dump)
        .await
        .with_context(|| format!("could not open {}", dump.display()))?;
    let mut stderr = Tail::default();
    let run = async {
        if let StartExecResults::Attached {
            mut output,
            mut input,
        } = docker.start_exec(&exec_id, None).await?
        {
            let feed = async {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = file.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    // The command may exit early (an error): stop feeding.
                    if input.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                let _ = input.shutdown().await;
                anyhow::Ok(())
            };
            let drain = async {
                while let Some(chunk) = output.next().await {
                    if let LogOutput::StdErr { message } = chunk? {
                        stderr.push(&message);
                    }
                }
                anyhow::Ok(())
            };
            let (fed, drained) = tokio::join!(feed, drain);
            fed?;
            drained?;
        }
        anyhow::Ok(())
    };
    tokio::time::timeout(HOOK_TIMEOUT, run)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the restore of service {} gave up after {} hours",
                hook.service,
                HOOK_TIMEOUT.as_secs() / 3600
            )
        })??;
    let exit_code = docker.inspect_exec(&exec_id).await?.exit_code.unwrap_or(-1);
    Ok((exit_code, stderr))
}

/// Run the restore command with the file at `dump` on stdin. A service that
/// was just started may not accept connections yet, so a failure is retried
/// for up to three minutes. No-op without a restore command.
pub async fn run_restore(docker: &Docker, hook: &BackupHook, dump: &Path) -> Result<()> {
    let Some(restore) = hook.restore.as_deref() else {
        return Ok(());
    };
    let started = tokio::time::Instant::now();
    let mut delay = Duration::from_secs(2);
    loop {
        let attempt = restore_once(docker, hook, restore, dump).await;
        let error = match attempt {
            Ok((0, _)) => return Ok(()),
            Ok((code, stderr)) => failure("restore", &hook.service, code, &stderr),
            Err(e) => e,
        };
        if started.elapsed() + delay > RESTORE_READY_TIMEOUT {
            return Err(error);
        }
        tracing::debug!(
            "Restore of service {} failed, retrying in {delay:?}: {error:#}",
            hook.service
        );
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(15));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_hooks_from_container_labels() {
        let hook = hook_from_labels(
            "c1",
            &labels(&[
                (SERVICE_LABEL, "db"),
                (DUMP_LABEL, "pg_dumpall -U postgres"),
                (RESTORE_LABEL, "psql -U postgres"),
            ]),
        );
        assert_eq!(
            hook,
            Some(BackupHook {
                service: "db".into(),
                container_id: "c1".into(),
                dump: "pg_dumpall -U postgres".into(),
                restore: Some("psql -U postgres".into()),
            })
        );
        // No dump, no hook; a bad restore command is ignored.
        assert_eq!(
            hook_from_labels("c1", &labels(&[(SERVICE_LABEL, "db")])),
            None
        );
        assert_eq!(
            hook_from_labels(
                "c1",
                &labels(&[
                    (SERVICE_LABEL, "db"),
                    (DUMP_LABEL, "redis-cli SAVE"),
                    (RESTORE_LABEL, "a\nb"),
                ])
            )
            .and_then(|h| h.restore),
            None
        );
        // Not a compose service, or an invalid dump: nothing.
        assert_eq!(hook_from_labels("c1", &labels(&[(DUMP_LABEL, "x")])), None);
        assert_eq!(
            hook_from_labels("c1", &labels(&[(SERVICE_LABEL, "db"), (DUMP_LABEL, " ")])),
            None
        );
    }

    #[test]
    fn stderr_keeps_only_the_tail() {
        let mut tail = Tail::default();
        tail.push(&vec![b'a'; STDERR_TAIL]);
        tail.push(b"the real error");
        assert_eq!(tail.0.len(), STDERR_TAIL);
        assert!(tail.text().ends_with("the real error"));
        let err = failure("dump", "db", 2, &tail).to_string();
        assert!(err.contains("service db exited with code 2"), "{err}");
        assert!(
            failure("dump", "db", 1, &Tail::default())
                .to_string()
                .ends_with("code 1")
        );
    }

    /// Dump and restore against a real container (needs Docker and the
    /// alpine image):
    ///   cargo nextest run -p tond-server --run-ignored only -E 'test(docker_backup_hooks)'
    #[tokio::test]
    #[ignore]
    async fn docker_backup_hooks_stream_dumps_and_restores() {
        use bollard::models::ContainerCreateBody;
        use bollard::query_parameters::{
            CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder,
        };

        let docker =
            Docker::connect_with_local_defaults().unwrap_or_else(|e| panic!("docker: {e}"));
        let name = format!("tond-hook-test-{}", uuid::Uuid::new_v4().simple());
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                ContainerCreateBody {
                    image: Some("alpine:3".to_string()),
                    cmd: Some(vec!["sleep".to_string(), "300".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        docker
            .start_container(
                &created.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .unwrap_or_else(|e| panic!("start: {e}"));

        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        // ~3 MiB: several stream chunks each way.
        let hook = BackupHook {
            service: "db".into(),
            container_id: created.id.clone(),
            dump: "mkdir -p /data && seq 1 400000 > /data/rows && cat /data/rows".into(),
            restore: Some("cat > /data/restored && cmp /data/rows /data/restored".into()),
        };
        let dump = dir.path().join("db.dump");
        let written = run_dump(&docker, &hook, &dump)
            .await
            .unwrap_or_else(|e| panic!("dump: {e:#}"));
        assert_eq!(
            written,
            std::fs::metadata(&dump).map(|m| m.len()).unwrap_or(0)
        );
        assert!(written > 2_000_000);
        run_restore(&docker, &hook, &dump)
            .await
            .unwrap_or_else(|e| panic!("restore: {e:#}"));

        let failing = BackupHook {
            dump: "echo boom >&2; exit 3".into(),
            ..hook.clone()
        };
        let err = run_dump(&docker, &failing, &dir.path().join("x"))
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("code 3") && err.contains("boom"), "{err}");

        let _ = docker
            .remove_container(
                &created.id,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
    }
}
