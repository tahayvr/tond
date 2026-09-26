// audit_log.rs — append-only, hash-chained audit log
//
// Every security-relevant event (sign-ins, 2FA, passwords, users, roles,
// tokens, settings, restores, terminal sessions, secret reveals, deletes) and
// every state change is written here with the *authenticated* actor: a user,
// an API token, the root-only local socket, or `system` for background work.
// The actor is built from the request's authentication (`Principal::actor`),
// never from a client-supplied field.
//
// Integrity:
//   Each row stores `hash = sha256(prev_hash || canonical(row))`, where
//   `canonical` is a JSON array of the row's fields in a fixed order and
//   `prev_hash` is the previous row's hash. Sequence numbers are assigned by
//   the writer (1, 2, 3, … without gaps), so deleting, reordering or editing a
//   row breaks verification. SQLite triggers (010_authz.sql) additionally
//   refuse UPDATEs, and DELETEs unless the janitor is pruning.
//
//   Retention (janitor.rs → `prune_before`) removes a prefix of the chain in
//   one transaction and records the last removed row's seq and hash in
//   `audit_state` as the checkpoint the retained chain starts from, then
//   appends an `audit_pruned` entry naming the checkpoint — so the chain stays
//   verifiable and the pruning itself is on the record.
//
//   `verify` walks the chain from the checkpoint; exposed as
//   GET /api/audit-log/verify and `tond audit verify`.
//
//   Limits: truncating the newest rows can't be detected from inside the
//   database; shipping entries to a remote sink (a planned follow-up) closes
//   that gap.
//
// Writes are serialised by a process-wide mutex (tond-server is the only
// writer) and each append runs in its own short transaction.

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tond_common::{AuditLog, AuditVerifyReport};

/// Hash the chain starts from (seeded into `audit_state` by 010_authz.sql).
#[cfg(test)]
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Serialises appends so sequence numbers and prev hashes never race.
static WRITE_LOCK: Mutex<()> = Mutex::const_new(());

/// Who performed an action, as recorded in the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditActor {
    /// "user" | "token" | "local_socket" | "system" | "anonymous"
    pub kind: &'static str,
    pub id: Option<String>,
    pub label: String,
    pub ip: Option<String>,
}

impl AuditActor {
    /// Background work (janitor, schedulers, webhooks) not tied to a caller.
    pub fn system(label: &str) -> Self {
        Self {
            kind: "system",
            id: None,
            label: label.to_string(),
            ip: None,
        }
    }

    /// An unauthenticated caller (e.g. a failed sign-in), identified by the
    /// name it claimed and its address.
    pub fn anonymous(label: &str, ip: Option<String>) -> Self {
        Self {
            kind: "anonymous",
            id: None,
            label: label.to_string(),
            ip,
        }
    }

    /// A signed-in user.
    pub fn user(id: &str, email: &str, ip: Option<String>) -> Self {
        Self {
            kind: "user",
            id: Some(id.to_string()),
            label: email.to_string(),
            ip,
        }
    }

    /// The root-only Unix socket (the `tond` CLI on the host).
    pub fn local_socket() -> Self {
        Self {
            kind: "local_socket",
            id: None,
            label: "local-socket".to_string(),
            ip: None,
        }
    }
}

/// What happened, to what.
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub failed: bool,
    pub detail: Option<String>,
}

impl AuditEntry {
    pub fn new(action: &str) -> Self {
        Self {
            action: action.to_string(),
            ..Self::default()
        }
    }

    /// The affected resource ("app", "database", "user", "token", …).
    pub fn resource(mut self, kind: &str, id: &str, name: &str) -> Self {
        self.resource_type = Some(kind.to_string());
        self.resource_id = Some(id.to_string());
        self.resource_name = Some(name.to_string());
        self
    }

    pub fn app(self, id: &str, name: &str) -> Self {
        self.resource("app", id, name)
    }

    /// A resource type without a specific row (e.g. "settings").
    pub fn kind(mut self, kind: &str) -> Self {
        self.resource_type = Some(kind.to_string());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn maybe_detail(mut self, detail: Option<&str>) -> Self {
        self.detail = detail.map(str::to_string);
        self
    }

    /// Mark the event as a failure (e.g. a wrong password).
    pub fn failed(mut self) -> Self {
        self.failed = true;
        self
    }
}

/// Append an entry. Non-fatal: a logging failure is logged and swallowed so
/// it never turns a completed action into an error response.
pub async fn record(db: &SqlitePool, actor: &AuditActor, entry: AuditEntry) {
    if let Err(e) = append(db, actor, &entry, &now_rfc3339()).await {
        tracing::error!(action = %entry.action, "failed to write audit log entry: {e}");
    }
}

/// `record` with an explicit timestamp (tests of retention).
#[cfg(test)]
pub async fn record_at(db: &SqlitePool, actor: &AuditActor, entry: AuditEntry, created_at: &str) {
    if let Err(e) = append(db, actor, &entry, created_at).await {
        panic!("failed to write audit log entry: {e}");
    }
}

/// Shorthand for app-scoped events (the most common kind).
pub async fn write_audit_log(
    db: &SqlitePool,
    app_id: Option<&str>,
    app_name: Option<&str>,
    action: &str,
    actor: &AuditActor,
    detail: Option<&str>,
) {
    let mut entry = AuditEntry::new(action).maybe_detail(detail);
    if app_id.is_some() || app_name.is_some() {
        entry.resource_type = Some("app".to_string());
        entry.resource_id = app_id.map(str::to_string);
        entry.resource_name = app_name.map(str::to_string);
    }
    record(db, actor, entry).await;
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Fields that make up a row's hash, in canonical order.
struct ChainFields<'a> {
    seq: i64,
    created_at: &'a str,
    actor_kind: &'a str,
    actor_id: Option<&'a str>,
    actor_label: &'a str,
    action: &'a str,
    resource_type: Option<&'a str>,
    resource_id: Option<&'a str>,
    resource_name: Option<&'a str>,
    outcome: &'a str,
    ip: Option<&'a str>,
    detail: Option<&'a str>,
}

impl ChainFields<'_> {
    fn canonical(&self) -> String {
        serde_json::json!([
            self.seq,
            self.created_at,
            self.actor_kind,
            self.actor_id,
            self.actor_label,
            self.action,
            self.resource_type,
            self.resource_id,
            self.resource_name,
            self.outcome,
            self.ip,
            self.detail,
        ])
        .to_string()
    }

    fn hash(&self, prev_hash: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_bytes());
        hasher.update(self.canonical().as_bytes());
        hex::encode(hasher.finalize())
    }
}

impl<'a> From<&'a AuditLog> for ChainFields<'a> {
    fn from(r: &'a AuditLog) -> Self {
        Self {
            seq: r.seq,
            created_at: &r.created_at,
            actor_kind: &r.actor_kind,
            actor_id: r.actor_id.as_deref(),
            actor_label: &r.actor_label,
            action: &r.action,
            resource_type: r.resource_type.as_deref(),
            resource_id: r.resource_id.as_deref(),
            resource_name: r.resource_name.as_deref(),
            outcome: &r.outcome,
            ip: r.ip.as_deref(),
            detail: r.detail.as_deref(),
        }
    }
}

/// The chain head: (seq, hash) of the newest row, or the checkpoint.
async fn head<'e, E>(executor: E) -> Result<(i64, String), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(
        "SELECT seq, hash FROM (
             SELECT seq, hash FROM audit_log
             UNION ALL
             SELECT checkpoint_seq, checkpoint_hash FROM audit_state WHERE id = 1
         ) ORDER BY seq DESC LIMIT 1",
    )
    .fetch_one(executor)
    .await
}

async fn append(
    db: &SqlitePool,
    actor: &AuditActor,
    entry: &AuditEntry,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let _guard = WRITE_LOCK.lock().await;
    let mut tx = db.begin().await?;
    let (last_seq, prev_hash) = head(&mut *tx).await?;
    let fields = ChainFields {
        seq: last_seq + 1,
        created_at,
        actor_kind: actor.kind,
        actor_id: actor.id.as_deref(),
        actor_label: &actor.label,
        action: &entry.action,
        resource_type: entry.resource_type.as_deref(),
        resource_id: entry.resource_id.as_deref(),
        resource_name: entry.resource_name.as_deref(),
        outcome: if entry.failed { "failure" } else { "success" },
        ip: actor.ip.as_deref(),
        detail: entry.detail.as_deref(),
    };
    let hash = fields.hash(&prev_hash);
    sqlx::query(
        "INSERT INTO audit_log (seq, created_at, actor_kind, actor_id, actor_label, action,
             resource_type, resource_id, resource_name, outcome, ip, detail, prev_hash, hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fields.seq)
    .bind(fields.created_at)
    .bind(fields.actor_kind)
    .bind(fields.actor_id)
    .bind(fields.actor_label)
    .bind(fields.action)
    .bind(fields.resource_type)
    .bind(fields.resource_id)
    .bind(fields.resource_name)
    .bind(fields.outcome)
    .bind(fields.ip)
    .bind(fields.detail)
    .bind(&prev_hash)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Rows verified per query while walking the chain.
const VERIFY_PAGE: i64 = 1_000;

/// Walk the whole retained chain and report the first problem, if any.
pub async fn verify(db: &SqlitePool) -> Result<AuditVerifyReport, sqlx::Error> {
    let (checkpoint_seq, checkpoint_hash): (i64, String) =
        sqlx::query_as("SELECT checkpoint_seq, checkpoint_hash FROM audit_state WHERE id = 1")
            .fetch_one(db)
            .await?;

    let mut report = AuditVerifyReport {
        ok: true,
        checked: 0,
        checkpoint_seq,
        first_seq: None,
        last_seq: None,
        broken_at: None,
        problem: None,
    };
    let mut expected_seq = checkpoint_seq + 1;
    let mut prev_hash = checkpoint_hash;

    loop {
        let rows: Vec<AuditLog> =
            sqlx::query_as("SELECT * FROM audit_log WHERE seq > ? ORDER BY seq ASC LIMIT ?")
                .bind(expected_seq - 1)
                .bind(VERIFY_PAGE)
                .fetch_all(db)
                .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            report.first_seq.get_or_insert(row.seq);
            let problem = if row.seq != expected_seq {
                Some(format!(
                    "entry {} is missing (next entry is {})",
                    expected_seq, row.seq
                ))
            } else if row.prev_hash != prev_hash {
                Some(format!(
                    "entry {} does not link to the entry before it",
                    row.seq
                ))
            } else if ChainFields::from(row).hash(&prev_hash) != row.hash {
                Some(format!(
                    "entry {} was modified after it was written",
                    row.seq
                ))
            } else {
                None
            };
            if let Some(problem) = problem {
                report.ok = false;
                report.broken_at = Some(row.seq.min(expected_seq));
                report.problem = Some(problem);
                return Ok(report);
            }
            report.checked += 1;
            report.last_seq = Some(row.seq);
            prev_hash = row.hash.clone();
            expected_seq += 1;
        }
    }

    // Rows below the checkpoint mean the checkpoint was moved without pruning.
    let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE seq <= ?")
        .bind(checkpoint_seq)
        .fetch_one(db)
        .await?;
    if stale > 0 {
        report.ok = false;
        report.problem = Some(format!(
            "{stale} entries sit before the retention checkpoint ({checkpoint_seq})"
        ));
    }
    Ok(report)
}

/// Remove entries created before `cutoff` (RFC 3339), at most `batch` per
/// transaction, keeping the chain verifiable. Returns the number removed.
pub async fn prune_before(db: &SqlitePool, cutoff: &str, batch: i64) -> Result<u64, sqlx::Error> {
    let mut removed = 0u64;
    let mut last_checkpoint = None;
    loop {
        let _guard = WRITE_LOCK.lock().await;
        let mut tx = db.begin().await?;
        // The newest row of this batch among the old ones: rows are appended in
        // time order, so "older than cutoff" is always a prefix of the chain.
        let through: Option<(i64, String)> = sqlx::query_as(
            "SELECT seq, hash FROM audit_log
             WHERE seq <= (SELECT MIN(seq) FROM audit_log) + ? - 1 AND created_at < ?
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(batch)
        .bind(cutoff)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((seq, hash)) = through else {
            break;
        };
        sqlx::query("UPDATE audit_state SET pruning = 1 WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query("DELETE FROM audit_log WHERE seq <= ?")
            .bind(seq)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query(
            "UPDATE audit_state SET pruning = 0, checkpoint_seq = ?, checkpoint_hash = ? WHERE id = 1",
        )
        .bind(seq)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        removed += deleted;
        last_checkpoint = Some((seq, hash));
        drop(_guard);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    if let Some((seq, hash)) = last_checkpoint {
        record(
            db,
            &AuditActor::system("janitor"),
            AuditEntry::new("audit_pruned")
                .kind("audit_log")
                .detail(format!(
                    "removed {removed} entries through #{seq}; chain continues from checkpoint {hash}"
                )),
        )
        .await;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for audit log tests: {err}"));
        db
    }

    fn admin() -> AuditActor {
        AuditActor::user(
            "user-1",
            "admin@example.com",
            Some("203.0.113.9".to_string()),
        )
    }

    async fn rows(db: &SqlitePool) -> Vec<AuditLog> {
        sqlx::query_as("SELECT * FROM audit_log ORDER BY seq")
            .fetch_all(db)
            .await
            .unwrap_or_else(|err| panic!("load audit rows: {err}"))
    }

    #[tokio::test]
    async fn record_persists_the_actor_resource_and_chains_hashes() {
        let db = test_pool().await;
        record(
            &db,
            &admin(),
            AuditEntry::new("deploy")
                .app("app-1", "frontend")
                .detail("manual"),
        )
        .await;
        record(
            &db,
            &AuditActor::local_socket(),
            AuditEntry::new("token_create"),
        )
        .await;

        let rows = rows(&db).await;
        assert_eq!(rows.len(), 2);
        let first = &rows[0];
        assert_eq!(first.seq, 1);
        assert_eq!(first.actor_kind, "user");
        assert_eq!(first.actor_id.as_deref(), Some("user-1"));
        assert_eq!(first.actor_label, "admin@example.com");
        assert_eq!(first.ip.as_deref(), Some("203.0.113.9"));
        assert_eq!(first.resource_type.as_deref(), Some("app"));
        assert_eq!(first.resource_name.as_deref(), Some("frontend"));
        assert_eq!(first.outcome, "success");
        assert_eq!(first.prev_hash, GENESIS_HASH);
        assert_eq!(rows[1].seq, 2);
        assert_eq!(rows[1].prev_hash, first.hash);
        assert_eq!(rows[1].actor_label, "local-socket");

        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checked, 2);
    }

    #[tokio::test]
    async fn failed_events_are_marked() {
        let db = test_pool().await;
        record(
            &db,
            &AuditActor::anonymous("who@example.com", None),
            AuditEntry::new("login_failed").failed(),
        )
        .await;
        assert_eq!(rows(&db).await[0].outcome, "failure");
    }

    #[tokio::test]
    async fn the_log_refuses_updates_and_deletes() {
        let db = test_pool().await;
        record(&db, &admin(), AuditEntry::new("deploy")).await;
        assert!(
            sqlx::query("UPDATE audit_log SET actor_label = 'someone-else'")
                .execute(&db)
                .await
                .is_err(),
            "updates must be refused"
        );
        assert!(
            sqlx::query("DELETE FROM audit_log")
                .execute(&db)
                .await
                .is_err(),
            "deletes outside retention must be refused"
        );
    }

    /// Rebuild the table without triggers to simulate an attacker with raw
    /// file access, then check that verification notices.
    async fn tamper(db: &SqlitePool, sql: &str) {
        sqlx::query("DROP TRIGGER audit_log_append_only_update")
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("drop trigger: {e}"));
        sqlx::query("DROP TRIGGER audit_log_append_only_delete")
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("drop trigger: {e}"));
        sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("tamper: {e}"));
    }

    async fn three_entries(db: &SqlitePool) {
        for action in ["one", "two", "three"] {
            record(db, &admin(), AuditEntry::new(action)).await;
        }
    }

    #[tokio::test]
    async fn verify_detects_an_edited_row() {
        let db = test_pool().await;
        three_entries(&db).await;
        tamper(
            &db,
            "UPDATE audit_log SET actor_label = 'mallory' WHERE seq = 2",
        )
        .await;
        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(!report.ok);
        assert_eq!(report.broken_at, Some(2));
        assert!(report.problem.unwrap_or_default().contains("modified"));
    }

    #[tokio::test]
    async fn verify_detects_a_deleted_row() {
        let db = test_pool().await;
        three_entries(&db).await;
        tamper(&db, "DELETE FROM audit_log WHERE seq = 2").await;
        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(!report.ok);
        assert_eq!(report.broken_at, Some(2));
        assert!(report.problem.unwrap_or_default().contains("missing"));
    }

    #[tokio::test]
    async fn verify_detects_a_rehashed_row() {
        // Recomputing the edited row's own hash still breaks the next link.
        let db = test_pool().await;
        three_entries(&db).await;
        let rows = rows(&db).await;
        let mut forged = rows[1].clone();
        forged.actor_label = "mallory".to_string();
        let new_hash = ChainFields::from(&forged).hash(&forged.prev_hash);
        tamper(
            &db,
            &format!(
                "UPDATE audit_log SET actor_label = 'mallory', hash = '{new_hash}' WHERE seq = 2"
            ),
        )
        .await;
        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(!report.ok);
        assert_eq!(report.broken_at, Some(3));
    }

    #[tokio::test]
    async fn pruning_keeps_the_chain_verifiable_and_is_itself_recorded() {
        let db = test_pool().await;
        three_entries(&db).await;
        // Everything so far is "old".
        let cutoff = (Utc::now() + chrono::Duration::seconds(5))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let removed = prune_before(&db, &cutoff, 2)
            .await
            .unwrap_or_else(|e| panic!("prune: {e}"));
        assert_eq!(removed, 3);

        let rows = rows(&db).await;
        assert_eq!(rows.len(), 1, "only the audit_pruned entry remains");
        assert_eq!(rows[0].action, "audit_pruned");
        assert_eq!(rows[0].seq, 4, "sequence numbers continue after pruning");

        record(&db, &admin(), AuditEntry::new("four")).await;
        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checkpoint_seq, 3);
        assert_eq!(report.first_seq, Some(4));
        assert_eq!(report.checked, 2);
    }

    #[tokio::test]
    async fn pruning_nothing_changes_nothing() {
        let db = test_pool().await;
        three_entries(&db).await;
        let removed = prune_before(&db, "2000-01-01T00:00:00.000Z", 100)
            .await
            .unwrap_or_else(|e| panic!("prune: {e}"));
        assert_eq!(removed, 0);
        assert_eq!(rows(&db).await.len(), 3);
    }

    #[tokio::test]
    async fn concurrent_writers_never_fork_the_chain() {
        let db = test_pool().await;
        let mut handles = Vec::new();
        for i in 0..20 {
            let db = db.clone();
            handles.push(tokio::spawn(async move {
                record(&db, &admin(), AuditEntry::new(&format!("event-{i}"))).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
        let report = verify(&db).await.unwrap_or_else(|e| panic!("verify: {e}"));
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checked, 20);
    }
}
