// backup.rs — Snapshot and BackupConfig models
//
// `Snapshot` records every backup taken: scheduled, manual, pre-deploy, and
// the pre-restore safety nets created before a full restore.
//
// `BackupConfig` is a single-row settings table (id = 1). It controls the
// scheduler, retention counts, and which StorageProvider (if any) receives
// remote uploads.

use serde::{Deserialize, Serialize};

// ── Database rows ─────────────────────────────────────────────────────────────

/// Raw row from the `snapshots` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    /// "pending" | "running" | "completed" | "failed"
    pub status: String,
    /// "scheduled" | "manual" | "pre_deploy" | "pre_restore"
    pub trigger: String,
    /// "full" | "app:{name}" | "database:{name}"
    pub scope: String,
    /// Set when trigger = "pre_deploy"
    pub trigger_app: Option<String>,
    /// FK → deployments.id (set for pre_deploy snapshots)
    pub deployment_id: Option<String>,
    /// Previous snapshot used as hard-link base for deduplication
    pub parent_snapshot_id: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    /// Bytes physically written (not counting hard-linked files)
    pub delta_bytes: Option<i64>,
    /// Total logical size if all hard links were resolved
    pub total_bytes: Option<i64>,
    /// Absolute path to local snapshot directory
    pub local_path: Option<String>,
    /// Object key on the remote storage provider (if uploaded)
    pub remote_key: Option<String>,
    /// FK → storage_providers.id (set if remote upload happened)
    pub storage_provider_id: Option<String>,
    /// SHA-256 of the encrypted archive in remote storage (if uploaded)
    pub checksum_sha256: Option<String>,
    pub error_message: Option<String>,
    pub notes: Option<String>,
    /// NULL (local only) | "uploading" | "uploaded" | "failed"
    pub remote_status: Option<String>,
    /// Why the last remote upload failed
    pub remote_error: Option<String>,
    /// Size of the encrypted archive in remote storage
    pub archive_bytes: Option<i64>,
}

/// Raw row from the `backup_config` table (always id = 1).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackupConfig {
    pub id: i64,
    pub enabled: i64,
    pub full_schedule_cron: String,
    pub full_retention_count: i64,
    pub pre_deploy_enabled: i64,
    pub pre_deploy_retention_per_app: i64,
    pub keep_local: i64,
    /// FK → storage_providers.id; None = local-only
    pub storage_provider_id: Option<String>,
}

// ── API response shapes ───────────────────────────────────────────────────────

/// Outbound representation of a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SnapshotResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub trigger: String,
    pub scope: String,
    pub trigger_app: Option<String>,
    pub deployment_id: Option<String>,
    pub parent_snapshot_id: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub delta_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub local_path: Option<String>,
    pub remote_key: Option<String>,
    pub storage_provider_id: Option<String>,
    pub checksum_sha256: Option<String>,
    pub error_message: Option<String>,
    pub notes: Option<String>,
    /// null (local only) | "uploading" | "uploaded" | "failed"
    #[serde(default)]
    pub remote_status: Option<String>,
    #[serde(default)]
    pub remote_error: Option<String>,
    /// Size of the encrypted archive in remote storage
    #[serde(default)]
    pub archive_bytes: Option<i64>,
}

impl From<Snapshot> for SnapshotResponse {
    fn from(s: Snapshot) -> Self {
        Self {
            id: s.id,
            name: s.name,
            status: s.status,
            trigger: s.trigger,
            scope: s.scope,
            trigger_app: s.trigger_app,
            deployment_id: s.deployment_id,
            parent_snapshot_id: s.parent_snapshot_id,
            created_at: s.created_at,
            completed_at: s.completed_at,
            delta_bytes: s.delta_bytes,
            total_bytes: s.total_bytes,
            local_path: s.local_path,
            remote_key: s.remote_key,
            storage_provider_id: s.storage_provider_id,
            checksum_sha256: s.checksum_sha256,
            error_message: s.error_message,
            notes: s.notes,
            remote_status: s.remote_status,
            remote_error: s.remote_error,
            archive_bytes: s.archive_bytes,
        }
    }
}

/// Outbound representation of the backup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackupConfigResponse {
    pub enabled: bool,
    pub full_schedule_cron: String,
    pub full_retention_count: i64,
    pub pre_deploy_enabled: bool,
    pub pre_deploy_retention_per_app: i64,
    pub keep_local: bool,
    pub storage_provider_id: Option<String>,
}

impl From<BackupConfig> for BackupConfigResponse {
    fn from(c: BackupConfig) -> Self {
        Self {
            enabled: c.enabled != 0,
            full_schedule_cron: c.full_schedule_cron,
            full_retention_count: c.full_retention_count,
            pre_deploy_enabled: c.pre_deploy_enabled != 0,
            pre_deploy_retention_per_app: c.pre_deploy_retention_per_app,
            keep_local: c.keep_local != 0,
            storage_provider_id: c.storage_provider_id,
        }
    }
}

// ── Request bodies ───────────────────────────────────────────────────────────

/// POST /api/v1/snapshots — trigger a manual Full snapshot
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateSnapshotRequest {
    /// Optional human-readable label for this snapshot
    pub name: Option<String>,
}

/// PUT /api/v1/snapshots/config — update backup configuration
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateBackupConfigRequest {
    pub enabled: Option<bool>,
    pub full_schedule_cron: Option<String>,
    pub full_retention_count: Option<i64>,
    pub pre_deploy_enabled: Option<bool>,
    pub pre_deploy_retention_per_app: Option<i64>,
    pub keep_local: Option<bool>,
    pub storage_provider_id: Option<String>,
}

/// POST /api/v1/snapshots/:id/restore-app
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestoreAppRequest {
    pub app_name: String,
}

/// POST /api/v1/snapshots/:id/restore-database
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestoreDatabaseRequest {
    pub db_name: String,
}

/// Reply to a database restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestoreDatabaseResponse {
    pub message: String,
    /// Backup of the data as it was just before the restore (to undo it)
    pub pre_restore_snapshot_id: String,
}
