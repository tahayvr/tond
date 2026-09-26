// updates/ — Tond updating itself
//
//   version.rs   release version numbers and ordering
//   releases.rs  the daily "is there a newer release?" check (GitHub releases
//                API, cached in `update_check`, folded into the janitor pass)
//   run.rs       starting an update or a rollback (snapshot, then a transient
//                systemd unit running update.sh) and reading its progress
//
// The HTTP side is handlers/updates.rs; the orchestration on the host
// (backup, swap, migrate, health check, automatic rollback) is release/update.sh.

pub mod releases;
pub mod run;
pub mod version;

use sqlx::SqlitePool;

use crate::audit_log::{AuditActor, AuditEntry, record};
use run::{Paths, RunKind, RunState, RunStatus};

/// The database schema version: the newest applied migration.
pub async fn schema_version(db: &SqlitePool) -> Option<i64> {
    sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(db)
        .await
        .ok()
        .flatten()
}

/// Audit-log entry for a finished run.
pub fn audit_entry(status: &RunStatus, state: RunState) -> AuditEntry {
    let (action, verb) = match status.kind {
        RunKind::Update => ("tond_update", "update"),
        RunKind::Rollback => ("tond_rollback", "rollback"),
    };
    let outcome = match state {
        RunState::Succeeded => "succeeded",
        RunState::RolledBack => "failed and was rolled back",
        RunState::Failed => "failed",
        RunState::Interrupted => "was interrupted",
        RunState::Running => "is running",
    };
    let mut detail = format!(
        "{verb} {} → {} {outcome}",
        status.from_version, status.to_version
    );
    if !status.message.is_empty() && state != RunState::Succeeded {
        detail.push_str(&format!(": {}", status.message));
    }
    if status.database_restored {
        detail.push_str(" (database restored from before the update)");
    }
    if let Some(id) = &status.snapshot_id {
        detail.push_str(&format!("; snapshot {id}"));
    }
    let entry = AuditEntry::new(action)
        .resource("system", "tond", "Tond")
        .detail(detail);
    if state == RunState::Succeeded {
        entry
    } else {
        entry.failed()
    }
}

/// Write the audit entry for the last run once it has finished. Called at
/// startup, from the janitor and when the status is read, so it lands soon
/// after the run ends; idempotent through `update_check.audited_run_id`
/// (which a restored database resets, so the outcome is still recorded).
pub async fn audit_finished_run(db: &SqlitePool, paths: &Paths) {
    let Some(status) = run::read_status(paths) else {
        return;
    };
    let state = run::effective_state(&status, run::pid_alive, chrono::Utc::now().timestamp());
    if !state.is_finished() {
        return;
    }
    let audited: Option<String> =
        sqlx::query_scalar("SELECT audited_run_id FROM update_check WHERE id = 1")
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .flatten();
    if audited.as_deref() == Some(status.id.as_str()) {
        return;
    }
    let claimed = sqlx::query(
        "UPDATE update_check SET audited_run_id = ? WHERE id = 1 AND audited_run_id IS NOT ?",
    )
    .bind(&status.id)
    .bind(&status.id)
    .execute(db)
    .await
    .map(|r| r.rows_affected() == 1)
    .unwrap_or(false);
    if !claimed {
        return;
    }
    let who = if status.actor.is_empty() {
        "update".to_string()
    } else {
        format!("update requested by {}", status.actor)
    };
    record(db, &AuditActor::system(&who), audit_entry(&status, state)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use run::{RunKind, RunState};

    fn status() -> RunStatus {
        RunStatus {
            id: "r1".into(),
            kind: RunKind::Update,
            state: RunState::RolledBack,
            step: "done".into(),
            from_version: "0.3.0".into(),
            to_version: "0.4.0".into(),
            actor: "admin@example.com".into(),
            snapshot_id: Some("snap-1".into()),
            started_at: 1,
            updated_at: 2,
            finished_at: Some(2),
            message: "tond-server did not become healthy".into(),
            pid: None,
            database_restored: true,
        }
    }

    #[test]
    fn audit_entries_describe_the_outcome() {
        let s = status();
        let entry = audit_entry(&s, RunState::RolledBack);
        assert_eq!(entry.action, "tond_update");
        assert!(entry.failed);
        let detail = entry.detail.unwrap_or_default();
        assert!(detail.contains("0.3.0 → 0.4.0 failed and was rolled back"));
        assert!(detail.contains("did not become healthy"));
        assert!(detail.contains("database restored"));
        assert!(detail.contains("snap-1"));

        let mut ok = status();
        ok.kind = RunKind::Rollback;
        ok.database_restored = false;
        let entry = audit_entry(&ok, RunState::Succeeded);
        assert_eq!(entry.action, "tond_rollback");
        assert!(!entry.failed);
        assert!(!entry.detail.unwrap_or_default().contains("healthy"));
    }

    #[tokio::test]
    async fn a_finished_run_is_audited_once() {
        let state = crate::state::AppState::for_tests().await;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let paths = Paths {
            update_dir: dir.path().join("update"),
            backups_dir: dir.path().join("backups"),
            bin_dir: dir.path().join("bin"),
        };
        run::write_status(&paths, &status()).unwrap_or_else(|e| panic!("{e}"));
        audit_finished_run(&state.db, &paths).await;
        audit_finished_run(&state.db, &paths).await;
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'tond_update'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn schema_version_is_the_newest_migration() {
        let state = crate::state::AppState::for_tests().await;
        let v = schema_version(&state.db).await;
        let newest = sqlx::migrate!("./migrations")
            .iter()
            .map(|m| m.version)
            .max();
        assert_eq!(v, newest);
    }
}
