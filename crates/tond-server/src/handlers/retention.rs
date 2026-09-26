// handlers/retention.rs — how long Tond keeps its history, and what it costs
//
//   get_retention()     GET /api/settings/retention — rules with bounds + database size
//   update_retention()  PUT /api/settings/retention — `{ "<key>": n, … }` within bounds
//
// The rules and their bounds are `tond_common::RETENTION_RULES`; the janitor
// (janitor.rs) reads the same settings keys every pass. The database size
// comes from SQLite's `dbstat` virtual table (per table, indexes folded into
// their table) and is cached for DB_SIZE_TTL because measuring it reads every
// page.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use sqlx::SqlitePool;
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::auth::{Authz, Principal, RateLimit, scope};
use crate::error::AppError;
use crate::janitor::RetentionPolicy;
use crate::state::AppState;

/// How long a database size measurement is reused.
const DB_SIZE_TTL: Duration = Duration::from_secs(300);

static DB_SIZE_CACHE: Mutex<Option<(Instant, DatabaseSize)>> = Mutex::new(None);

/// The rules with their current values.
pub fn retention_fields(policy: &RetentionPolicy) -> Vec<RetentionField> {
    RETENTION_RULES
        .iter()
        .map(|rule| RetentionField {
            key: rule.key.to_string(),
            value: policy.get(rule.key).unwrap_or(rule.default),
            default: rule.default,
            min: rule.min,
            max: rule.max,
            unit: rule.unit.to_string(),
        })
        .collect()
}

/// Measure the database file, its WAL and (via dbstat) each table.
pub async fn measure_database(db: &SqlitePool) -> Result<DatabaseSize, sqlx::Error> {
    let files: Vec<(i64, String, String)> =
        sqlx::query_as("PRAGMA database_list").fetch_all(db).await?;
    let path = files
        .into_iter()
        .find(|(_, name, _)| name == "main")
        .map(|(_, _, file)| file)
        .unwrap_or_default();
    let size_of = |p: &str| {
        if p.is_empty() {
            0
        } else {
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
        }
    };
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size").fetch_one(db).await?;
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(db)
        .await?;
    let free_pages: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(db)
        .await?;
    let file_bytes = match size_of(&path) {
        0 => u64::try_from(page_size * page_count).unwrap_or(0),
        n => n,
    };

    // dbstat is compiled into the bundled SQLite; without it, no breakdown.
    let btrees: Vec<(String, i64)> =
        sqlx::query_as("SELECT name, pgsize FROM dbstat WHERE aggregate = TRUE")
            .fetch_all(db)
            .await
            .unwrap_or_default();
    let owners: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT name, tbl_name FROM sqlite_schema")
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();
    let mut per_table: BTreeMap<String, u64> = BTreeMap::new();
    for (name, bytes) in btrees {
        let table = owners.get(&name).cloned().unwrap_or(name);
        *per_table.entry(table).or_default() += u64::try_from(bytes).unwrap_or(0);
    }
    let mut tables: Vec<TableSize> = per_table
        .into_iter()
        .map(|(name, bytes)| TableSize { name, bytes })
        .collect();
    tables.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));

    Ok(DatabaseSize {
        file_bytes,
        wal_bytes: size_of(&format!("{path}-wal")),
        free_bytes: u64::try_from(page_size * free_pages).unwrap_or(0),
        tables,
        measured_at: chrono::Utc::now().to_rfc3339(),
    })
}

async fn cached_database_size(db: &SqlitePool) -> Option<DatabaseSize> {
    if let Some((at, size)) = DB_SIZE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        && at.elapsed() < DB_SIZE_TTL
    {
        return Some(size.clone());
    }
    match measure_database(db).await {
        Ok(size) => {
            *DB_SIZE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((Instant::now(), size.clone()));
            Some(size)
        }
        Err(e) => {
            tracing::warn!("Could not measure the database: {e}");
            None
        }
    }
}

pub async fn get_retention(
    _rate: RateLimit,
    _auth: Principal,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let policy = RetentionPolicy::load(&state.db).await;
    let response = RetentionResponse {
        fields: retention_fields(&policy),
        database: cached_database_size(&state.db).await,
    };
    Ok((StatusCode::OK, Json(ApiResponse::ok(response))))
}

/// Check a requested change against the rules. Returns the (key, value)
/// pairs to store, or a message naming the first bad value.
pub fn validate_changes(
    changes: &BTreeMap<String, i64>,
) -> Result<Vec<(&'static str, i64)>, String> {
    let mut pairs = Vec::new();
    for (key, value) in changes {
        let rule = retention_rule(key).ok_or_else(|| format!("Unknown setting '{key}'"))?;
        if !(rule.min..=rule.max).contains(value) {
            return Err(format!(
                "{key} must be between {} and {} {}",
                rule.min, rule.max, rule.unit
            ));
        }
        pairs.push((rule.key, *value));
    }
    Ok(pairs)
}

pub async fn update_retention(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(changes): Json<BTreeMap<String, i64>>,
) -> Result<impl IntoResponse, AppError> {
    let pairs = validate_changes(&changes).map_err(TondError::InvalidInput)?;
    let before = RetentionPolicy::load(&state.db).await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = state.db.begin().await?;
    for (key, value) in &pairs {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value.to_string())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let changed: Vec<String> = pairs
        .iter()
        .filter(|(k, v)| before.get(k) != Some(*v))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !changed.is_empty() {
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("retention_update")
                .resource("settings", "retention", "retention")
                .detail(changed.join(" ")),
        )
        .await;
    }

    let policy = RetentionPolicy::load(&state.db).await;
    let response = RetentionResponse {
        fields: retention_fields(&policy),
        database: cached_database_size(&state.db).await,
    };
    Ok((StatusCode::OK, Json(ApiResponse::ok(response))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_checked_against_the_rules() {
        let mut ok = BTreeMap::new();
        ok.insert("retention_audit_log_days".to_string(), 365);
        assert_eq!(
            validate_changes(&ok),
            Ok(vec![("retention_audit_log_days", 365)])
        );

        let mut low = BTreeMap::new();
        low.insert("retention_audit_log_days".to_string(), 5);
        assert!(
            validate_changes(&low).is_err_and(|e| e.contains("between 30 and 3650")),
            "the audit log keeps at least 30 days"
        );

        let mut unknown = BTreeMap::new();
        unknown.insert("retention_everything".to_string(), 5);
        assert!(validate_changes(&unknown).is_err());
    }

    #[tokio::test]
    async fn update_stores_values_audits_and_returns_fields_with_the_database_size() {
        let state = AppState::for_tests().await;
        let mut body = BTreeMap::new();
        body.insert("retention_deploy_logs_days".to_string(), 60);
        let response = update_retention(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(body),
        )
        .await
        .unwrap_or_else(|_| panic!("update should succeed"))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let policy = RetentionPolicy::load(&state.db).await;
        assert_eq!(policy.deploy_log_days, 60);
        let fields = retention_fields(&policy);
        assert_eq!(fields.len(), RETENTION_RULES.len());
        assert!(
            fields
                .iter()
                .any(|f| f.key == "retention_deploy_logs_days" && f.value == 60 && f.default == 90)
        );

        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'retention_update'
               AND detail = 'retention_deploy_logs_days=60'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("audit: {e}"));
        assert_eq!(audited, 1);

        let size = measure_database(&state.db)
            .await
            .unwrap_or_else(|e| panic!("measure: {e}"));
        assert!(size.file_bytes > 0);
        assert!(
            size.tables.iter().any(|t| t.name == "audit_log"),
            "dbstat breaks the size down per table: {:?}",
            size.tables
        );
        assert!(
            !size
                .tables
                .iter()
                .any(|t| t.name.starts_with("idx_audit_log")),
            "indexes are folded into their table"
        );
    }
}
