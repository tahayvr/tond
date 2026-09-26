// cron.rs — cron job and cron job run types

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Phase 6.4: Cron Jobs ──

/// A scheduled job that runs a Docker container on a cron schedule.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CronJob {
    pub id: String,
    /// User-chosen unique name
    pub name: String,
    /// Standard cron expression (5-field), e.g. "0 * * * *"
    pub schedule: String,
    /// Docker image to run
    pub image: String,
    /// Optional command override (stored as a JSON array string, or plain string)
    pub command: Option<String>,
    /// Optional project association
    pub project_id: Option<String>,
    /// 1 = enabled, 0 = paused
    pub enabled: i64,
    /// ISO-8601 timestamp of last execution
    pub last_run_at: Option<String>,
    /// ISO-8601 timestamp of next scheduled execution
    pub next_run_at: Option<String>,
    /// "idle" | "running" | "failed"
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Maximum wall-clock seconds the container may run. NULL = no timeout.
    pub timeout_seconds: Option<i64>,
    /// Environment variables: a JSON object, stored encrypted by the server.
    /// NULL = none. Never serialized.
    #[serde(skip_serializing)]
    pub env: Option<String>,
    /// Denormalised last-run outcome: "success" | "failed" | NULL (never run).
    pub last_run_status: Option<String>,
    /// Concurrency policy: "skip" (default) or "allow".
    pub concurrency_policy: String,
    /// Memory limit in MiB. NULL = no limit.
    pub memory_mb: Option<i64>,
    /// Docker CPU shares (relative weight). NULL = default (1024).
    pub cpu_shares: Option<i64>,
    /// IANA timezone for schedule evaluation. NULL = UTC.
    pub timezone: Option<String>,
    /// Number of automatic retries on non-zero exit. 0 = no retries.
    pub max_retries: i64,
    /// Seconds to wait between retries.
    pub retry_delay_seconds: i64,
    /// Volume mounts: JSON array of `{"host_path":"...","container_path":"..."}`. NULL = none.
    pub volumes: Option<String>,
}

impl CronJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        schedule: String,
        image: String,
        command: Option<String>,
        project_id: Option<String>,
        timeout_seconds: Option<i64>,
        env: Option<String>,
        concurrency_policy: String,
        memory_mb: Option<i64>,
        cpu_shares: Option<i64>,
        timezone: Option<String>,
        max_retries: i64,
        retry_delay_seconds: i64,
        volumes: Option<String>,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            schedule,
            image,
            command,
            project_id,
            enabled: 1,
            last_run_at: None,
            next_run_at: None,
            status: "idle".to_string(),
            created_at: now.clone(),
            updated_at: now,
            timeout_seconds,
            env,
            last_run_status: None,
            concurrency_policy,
            memory_mb,
            cpu_shares,
            timezone,
            max_retries,
            retry_delay_seconds,
            volumes,
        }
    }
}

/// API view of a cron job (enabled is returned as bool for ergonomics).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CronJobResponse {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub image: String,
    pub command: Option<String>,
    pub project_id: Option<String>,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Maximum wall-clock seconds the container may run. null = no timeout.
    pub timeout_seconds: Option<i64>,
    /// Environment variable keys with masked values (values are secrets).
    #[serde(default)]
    pub env: Vec<crate::EnvVarEntry>,
    /// Denormalised last-run outcome: "success" | "failed" | null (never run).
    pub last_run_status: Option<String>,
    /// Concurrency policy: "skip" or "allow".
    pub concurrency_policy: String,
    /// Memory limit in MiB. null = no limit.
    pub memory_mb: Option<i64>,
    /// Docker CPU shares. null = default.
    pub cpu_shares: Option<i64>,
    /// IANA timezone. null = UTC.
    pub timezone: Option<String>,
    /// Max automatic retries on non-zero exit.
    pub max_retries: i64,
    /// Seconds between retries.
    pub retry_delay_seconds: i64,
    /// Volume mounts. null = none.
    pub volumes: Option<Vec<CronVolume>>,
}

/// A cron job bind mount. The host side is chosen by the server: the job's own
/// directory `<data>/volumes/_cron/<job-id>/<name>`, or — when `app` is set —
/// that app's volume `<data>/volumes/<app>/<name>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CronVolume {
    /// Share a volume of this app instead of using the job's own directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Directory name of the volume
    pub name: String,
    /// Absolute host path, computed by the server (read-only)
    pub host_path: String,
    /// Mount point inside the container
    pub container_path: String,
}

/// A cron volume as sent by clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CronVolumeRequest {
    /// Share a volume of this existing app (e.g. to back up its data)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Volume directory name (default: derived from `container_path`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Only accepted when it already points inside the allowed directory; prefer `name`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_path: Option<String>,
    /// Mount point inside the container
    pub container_path: String,
}

impl CronJobResponse {
    /// Build a response from a job and its (decrypted) env.
    pub fn new(j: CronJob, env: &std::collections::BTreeMap<String, String>) -> Self {
        let env = crate::masked_env_entries(env);
        let volumes = j
            .volumes
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        Self {
            id: j.id,
            name: j.name,
            schedule: j.schedule,
            image: j.image,
            command: j.command,
            project_id: j.project_id,
            enabled: j.enabled != 0,
            last_run_at: j.last_run_at,
            next_run_at: j.next_run_at,
            status: j.status,
            created_at: j.created_at,
            updated_at: j.updated_at,
            timeout_seconds: j.timeout_seconds,
            env,
            last_run_status: j.last_run_status,
            concurrency_policy: j.concurrency_policy,
            memory_mb: j.memory_mb,
            cpu_shares: j.cpu_shares,
            timezone: j.timezone,
            max_retries: j.max_retries,
            retry_delay_seconds: j.retry_delay_seconds,
            volumes,
        }
    }
}

/// One execution record for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CronJobRun {
    pub id: String,
    pub cron_job_id: String,
    /// "running" | "success" | "failed"
    pub status: String,
    pub exit_code: Option<i64>,
    /// Last N lines of container stdout+stderr
    pub output: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Retry attempt number: 1 = first attempt, 2 = first retry, etc.
    pub attempt: i64,
}

/// POST /api/v1/cron request body
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateCronJobRequest {
    pub name: String,
    pub schedule: String,
    pub image: String,
    /// Optional command override, e.g. ["curl", "https://example.com"]
    pub command: Option<Vec<String>>,
    pub project_id: Option<String>,
    /// Maximum wall-clock seconds the container may run. None = no timeout.
    pub timeout_seconds: Option<i64>,
    /// Environment variables, e.g. {"KEY": "value"}
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Concurrency policy: "skip" (default) or "allow"
    pub concurrency_policy: Option<String>,
    /// Memory limit in MiB. None = no limit.
    pub memory_mb: Option<i64>,
    /// Docker CPU shares. None = default (1024).
    pub cpu_shares: Option<i64>,
    /// IANA timezone for schedule. None = UTC.
    pub timezone: Option<String>,
    /// Max automatic retries on non-zero exit. None = 0.
    pub max_retries: Option<i64>,
    /// Seconds between retries. None = 60.
    pub retry_delay_seconds: Option<i64>,
    /// Volume mounts: [{"name":"data","container_path":"/data"}] (optionally with "app")
    pub volumes: Option<Vec<CronVolumeRequest>>,
}

/// PATCH /api/v1/cron/{id} request body (all fields optional)
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateCronJobRequest {
    /// Rename the job. Returns 409 if the new name is already taken.
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub image: Option<String>,
    /// Send an empty array [] to clear the command override (revert to image default).
    pub command: Option<Vec<String>>,
    pub enabled: Option<bool>,
    /// Maximum wall-clock seconds the container may run. None = no change.
    /// Pass `Some(None)` to clear (remove timeout), `Some(Some(n))` to set.
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<i64>"))]
    pub timeout_seconds: Option<Option<i64>>,
    /// Replace all environment variables. None = no change; Some({}) = clear all.
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Environment variables to add or overwrite (applied after `env`).
    #[serde(default)]
    pub env_set: std::collections::BTreeMap<String, String>,
    /// Environment variables to remove (applied after `env_set`).
    #[serde(default)]
    pub env_unset: Vec<String>,
    /// Link/unlink a project. None = no change; Some(None) = unlink; Some(Some(id)) = link.
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub project_id: Option<Option<String>>,
    /// Concurrency policy: "skip" or "allow". None = no change.
    pub concurrency_policy: Option<String>,
    /// Memory limit in MiB. None = no change. Some(None) = clear limit.
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<i64>"))]
    pub memory_mb: Option<Option<i64>>,
    /// Docker CPU shares. None = no change. Some(None) = clear (use default).
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<i64>"))]
    pub cpu_shares: Option<Option<i64>>,
    /// IANA timezone. None = no change. Some(None) = revert to UTC.
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub timezone: Option<Option<String>>,
    /// Max automatic retries on non-zero exit. None = no change.
    pub max_retries: Option<i64>,
    /// Seconds between retries. None = no change.
    pub retry_delay_seconds: Option<i64>,
    /// Volume mounts. None = no change. Some([]) = clear all mounts.
    pub volumes: Option<Vec<CronVolumeRequest>>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn cron_job_new_sets_documented_defaults() {
        let job = CronJob::new(
            "nightly-backup".to_string(),
            "0 0 * * *".to_string(),
            "alpine:latest".to_string(),
            Some("[\"echo\",\"hello\"]".to_string()),
            Some("project-1".to_string()),
            Some(60),
            Some("{\"KEY\":\"value\"}".to_string()),
            "skip".to_string(),
            Some(256),
            Some(1024),
            Some("UTC".to_string()),
            2,
            30,
            Some("[{\"host_path\":\"/tmp\",\"container_path\":\"/data\"}]".to_string()),
        );

        assert_eq!(
            job.name, "nightly-backup",
            "new cron jobs should preserve their name"
        );
        assert_eq!(job.enabled, 1, "new cron jobs should start enabled");
        assert_eq!(job.status, "idle", "new cron jobs should start idle");
        assert_eq!(
            job.last_run_at, None,
            "new cron jobs should not report a last run before first execution"
        );
        assert_eq!(
            job.next_run_at, None,
            "new cron jobs should not have a next_run_at until scheduling computes it"
        );
    }

    #[test]
    fn cron_job_response_parses_json_fields_and_bool_flags() {
        let job = CronJob {
            id: "job-1".to_string(),
            name: "nightly-backup".to_string(),
            schedule: "0 0 * * *".to_string(),
            image: "alpine:latest".to_string(),
            command: Some("[\"echo\",\"hello\"]".to_string()),
            project_id: Some("project-1".to_string()),
            enabled: 1,
            last_run_at: Some("2026-01-01T00:00:00Z".to_string()),
            next_run_at: Some("2026-01-02T00:00:00Z".to_string()),
            status: "idle".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            timeout_seconds: Some(60),
            env: Some("{\"KEY\":\"value\"}".to_string()),
            last_run_status: Some("success".to_string()),
            concurrency_policy: "skip".to_string(),
            memory_mb: Some(256),
            cpu_shares: Some(1024),
            timezone: Some("UTC".to_string()),
            max_retries: 1,
            retry_delay_seconds: 30,
            volumes: Some(
                "[{\"name\":\"data\",\"host_path\":\"/opt/tond/data/volumes/_cron/job-1/data\",\"container_path\":\"/data\"}]"
                    .to_string(),
            ),
        };

        let env = std::collections::BTreeMap::from([("KEY".to_string(), "value".to_string())]);
        let response = CronJobResponse::new(job, &env);

        assert!(
            response.enabled,
            "CronJobResponse should convert non-zero enabled flags to true"
        );
        assert_eq!(
            response.env,
            vec![crate::EnvVarEntry {
                key: "KEY".to_string(),
                masked: crate::ENV_VALUE_MASK.to_string(),
            }],
            "CronJobResponse should list env keys with masked values"
        );
        assert_eq!(
            response.volumes,
            Some(vec![CronVolume {
                app: None,
                name: "data".to_string(),
                host_path: "/opt/tond/data/volumes/_cron/job-1/data".to_string(),
                container_path: "/data".to_string(),
            }]),
            "CronJobResponse should deserialize stored volume JSON into typed volumes"
        );
    }

    #[test]
    fn cron_job_response_ignores_invalid_json_fields() {
        let job = CronJob {
            id: "job-1".to_string(),
            name: "nightly-backup".to_string(),
            schedule: "0 0 * * *".to_string(),
            image: "alpine:latest".to_string(),
            command: None,
            project_id: None,
            enabled: 0,
            last_run_at: None,
            next_run_at: None,
            status: "idle".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            timeout_seconds: None,
            env: Some("not json".to_string()),
            last_run_status: None,
            concurrency_policy: "skip".to_string(),
            memory_mb: None,
            cpu_shares: None,
            timezone: None,
            max_retries: 0,
            retry_delay_seconds: 60,
            volumes: Some("still not json".to_string()),
        };

        let response = CronJobResponse::new(job, &Default::default());

        assert!(
            !response.enabled,
            "CronJobResponse should convert zero enabled flags to false"
        );
        assert!(response.env.is_empty());
        assert_eq!(
            response.volumes, None,
            "invalid volumes JSON should be dropped rather than causing response conversion to fail"
        );
    }

    #[test]
    fn update_cron_job_request_preserves_nested_option_patch_semantics() {
        let request: UpdateCronJobRequest = serde_json::from_value(serde_json::json!({
            "timeout_seconds": null,
            "project_id": null,
            "memory_mb": null,
            "cpu_shares": 1024,
            "timezone": "Europe/Berlin"
        }))
        .unwrap_or_else(|err| panic!("failed to deserialize UpdateCronJobRequest: {err}"));

        assert_eq!(
            request.timeout_seconds,
            Some(None),
            "an explicit null timeout_seconds should deserialize as Some(None) so handlers can clear it"
        );
        assert_eq!(
            request.project_id,
            Some(None),
            "an explicit null project_id should deserialize as Some(None) so handlers can unlink the project"
        );
        assert_eq!(
            request.memory_mb,
            Some(None),
            "an explicit null memory_mb should deserialize as Some(None) so handlers can clear the memory limit"
        );
        assert_eq!(
            request.cpu_shares,
            Some(Some(1024)),
            "a concrete cpu_shares value should deserialize as Some(Some(value))"
        );
        assert_eq!(
            request.timezone,
            Some(Some("Europe/Berlin".to_string())),
            "a concrete timezone value should deserialize as Some(Some(value))"
        );
    }

    #[test]
    fn create_cron_job_request_round_trips_optional_fields() {
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "value".to_string());

        let request = CreateCronJobRequest {
            name: "nightly-backup".to_string(),
            schedule: "0 0 * * *".to_string(),
            image: "alpine:latest".to_string(),
            command: Some(vec!["echo".to_string(), "hello".to_string()]),
            project_id: Some("project-1".to_string()),
            timeout_seconds: Some(60),
            env: Some(env),
            concurrency_policy: Some("skip".to_string()),
            memory_mb: Some(256),
            cpu_shares: Some(1024),
            timezone: Some("UTC".to_string()),
            max_retries: Some(2),
            retry_delay_seconds: Some(30),
            volumes: Some(vec![CronVolumeRequest {
                app: Some("web".to_string()),
                name: Some("data".to_string()),
                host_path: None,
                container_path: "/data".to_string(),
            }]),
        };

        let roundtrip: CreateCronJobRequest = serde_json::from_value(
            serde_json::to_value(&request)
                .unwrap_or_else(|err| panic!("failed to serialize CreateCronJobRequest: {err}")),
        )
        .unwrap_or_else(|err| panic!("failed to deserialize CreateCronJobRequest: {err}"));

        assert_eq!(
            roundtrip.command,
            Some(vec!["echo".to_string(), "hello".to_string()]),
            "CreateCronJobRequest should preserve command overrides through serde round-trips"
        );
        assert_eq!(
            roundtrip.max_retries,
            Some(2),
            "CreateCronJobRequest should preserve retry settings through serde round-trips"
        );
        let volumes = roundtrip.volumes.unwrap_or_default();
        assert_eq!(
            (
                volumes.len(),
                volumes[0].app.as_deref(),
                volumes[0].name.as_deref(),
                volumes[0].container_path.as_str()
            ),
            (1, Some("web"), Some("data"), "/data"),
            "CreateCronJobRequest should preserve volume mounts through serde round-trips"
        );
    }
}
