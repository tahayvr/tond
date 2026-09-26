// deploy_follow.rs — `tond apps deploy --wait` follows the deployment it started
//
// Reads the deployment stream (GET /api/v1/apps/{id}/deployments/{dep}/stream):
// a snapshot with the row and its log so far, then `log` lines and
// `deployment` updates until the server closes the socket shortly after the
// deployment finishes. Phase changes print as
//
//   ▸ Build
//     │ Step 3/7 : RUN npm ci
//   ✓ Build (1 min 4 s)
//
// With --json the same progress is NDJSON on stdout, one event per line:
//   {"event":"log", "message":…, …the server's log fields}
//   {"event":"phase","phase":"build","status":"running","label":…,"started_at":…,"finished_at":…}
//   {"event":"result","app":…,"deployment_id":…,"status":"succeeded|failed|superseded|cancelled|unknown",
//    "exit_code":N,"error":{code,message,hint}|null,"failure":{…DeployFailure}|null}
//
// and the command ends with the outcome (`finish`): exit 0 on success; a
// failed build/clone is BUILD_FAILED and any other failure DEPLOY_FAILED
// (exit 6); superseded/cancelled exit 5; a stream that ended before the
// outcome was known exits 1.

use std::collections::HashMap;

use anyhow::Result;
use chrono::DateTime;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tond_common::{DeployFailure, DeployPhase, PhaseRecord, PhaseStatus, code, exit};

use crate::client::TondClient;
use crate::error::{CliError, local_code};
use crate::output::{Output, outln};

/// Where the deployment ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed(Option<DeployFailure>, Option<String>),
    /// "superseded" or "cancelled".
    Stopped(String),
    /// The stream ended before the deployment finished.
    Unknown,
}

impl Outcome {
    fn status(&self) -> &str {
        match self {
            Outcome::Succeeded => "succeeded",
            Outcome::Failed(..) => "failed",
            Outcome::Stopped(s) => s,
            Outcome::Unknown => "unknown",
        }
    }
}

fn phase_name(record: &PhaseRecord) -> String {
    if let Some(label) = record.label.as_deref().filter(|l| !l.is_empty()) {
        return label.to_string();
    }
    match record.phase {
        DeployPhase::Clone => "Fetch source",
        DeployPhase::Build => "Build",
        DeployPhase::Start => "Start",
        DeployPhase::Health => "Health check",
        DeployPhase::Release => "Switch traffic",
    }
    .to_string()
}

/// "4 s", "1 min 4 s".
pub fn format_secs(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs} s")
    } else if secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{} min {} s", secs / 60, secs % 60)
    }
}

fn seconds_between(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let s = DateTime::parse_from_rfc3339(start?).ok()?;
    let e = DateTime::parse_from_rfc3339(end?).ok()?;
    Some((e - s).num_seconds())
}

/// The phases whose status changed since `seen` (which is updated) and are
/// worth reporting (running, succeeded, failed).
pub fn phase_changes<'a>(
    seen: &mut HashMap<DeployPhase, PhaseStatus>,
    phases: &'a [PhaseRecord],
) -> Vec<&'a PhaseRecord> {
    phases
        .iter()
        .filter(|record| seen.insert(record.phase, record.status) != Some(record.status))
        .filter(|record| {
            matches!(
                record.status,
                PhaseStatus::Running | PhaseStatus::Succeeded | PhaseStatus::Failed
            )
        })
        .collect()
}

/// Human line for a phase change.
fn phase_line(record: &PhaseRecord) -> String {
    let name = phase_name(record);
    match record.status {
        PhaseStatus::Running => format!("▸ {name}"),
        PhaseStatus::Succeeded => {
            let took = seconds_between(record.started_at.as_deref(), record.finished_at.as_deref())
                .map(|s| format!(" ({})", format_secs(s)))
                .unwrap_or_default();
            format!("✓ {name}{took}")
        }
        _ => format!("✗ {name}"),
    }
}

#[cfg(test)]
/// Lines to print for the phases that changed since `seen`, which is updated.
pub fn phase_updates(
    seen: &mut HashMap<DeployPhase, PhaseStatus>,
    phases: &[PhaseRecord],
) -> Vec<String> {
    phase_changes(seen, phases)
        .into_iter()
        .map(phase_line)
        .collect()
}

/// The outcome implied by a deployment JSON object, if it has finished.
pub fn outcome_of(deployment: &Value) -> Option<Outcome> {
    let status = deployment.get("status").and_then(Value::as_str)?;
    match status {
        "succeeded" => Some(Outcome::Succeeded),
        "failed" => Some(Outcome::Failed(
            deployment
                .get("failure")
                .cloned()
                .and_then(|f| serde_json::from_value(f).ok()),
            deployment
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        )),
        "superseded" | "cancelled" => Some(Outcome::Stopped(status.to_string())),
        _ => None,
    }
}

fn phases_of(deployment: &Value) -> Vec<PhaseRecord> {
    deployment
        .get("phases")
        .cloned()
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default()
}

fn emit_log(out: &Output, line: &Value) -> Result<()> {
    let message = line.get("message").and_then(Value::as_str).unwrap_or("");
    for part in message.lines() {
        outln!(out, "  │ {part}");
    }
    let mut event = match line {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    event.remove("type");
    event.insert("event".into(), json!("log"));
    out.event(&event)
}

fn emit_phases(
    out: &Output,
    seen: &mut HashMap<DeployPhase, PhaseStatus>,
    deployment: &Value,
) -> Result<()> {
    let phases = phases_of(deployment);
    for record in phase_changes(seen, &phases) {
        outln!(out, "{}", phase_line(record));
        out.event(&json!({
            "event": "phase",
            "phase": record.phase,
            "status": record.status,
            "label": record.label,
            "started_at": record.started_at,
            "finished_at": record.finished_at,
        }))?;
    }
    Ok(())
}

/// Follow `deployment_id` of `app` until it finishes; print progress.
pub async fn follow(
    client: &TondClient,
    out: &Output,
    app: &str,
    deployment_id: &str,
) -> Result<Outcome> {
    let mut stream = client
        .ws_messages(&format!("/apps/{app}/deployments/{deployment_id}/stream"))
        .await?;
    let mut seen: HashMap<DeployPhase, PhaseStatus> = HashMap::new();
    let mut outcome = Outcome::Unknown;

    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(frame)) => {
                if let Some(err) = frame
                    .as_ref()
                    .and_then(|f| CliError::from_close(u16::from(f.code), &f.reason))
                {
                    return Err(err.into());
                }
                break;
            }
            Ok(_) => continue,
            // Fall through to the final lookup below.
            Err(_) => break,
        };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match frame.get("type").and_then(Value::as_str) {
            Some("snapshot") => {
                let deployment = frame.get("deployment").cloned().unwrap_or_default();
                // Replay the history so the output reads top to bottom.
                for line in frame
                    .get("logs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    emit_log(out, line)?;
                }
                emit_phases(out, &mut seen, &deployment)?;
                if let Some(o) = outcome_of(&deployment) {
                    outcome = o;
                }
            }
            Some("log") => emit_log(out, &frame)?,
            Some("deployment") => {
                let deployment = frame.get("deployment").cloned().unwrap_or_default();
                emit_phases(out, &mut seen, &deployment)?;
                if let Some(o) = outcome_of(&deployment) {
                    outcome = o;
                }
            }
            _ => {}
        }
    }

    if outcome == Outcome::Unknown
        && let Ok(detail) = client.get_deployment(app, deployment_id).await
        && let Some(o) = detail.get("deployment").and_then(outcome_of)
    {
        outcome = o;
    }
    Ok(outcome)
}

/// Did the deployment fail while fetching or building the source?
fn is_build_failure(failure: &DeployFailure) -> bool {
    matches!(
        failure.phase,
        Some(DeployPhase::Clone) | Some(DeployPhase::Build)
    ) || matches!(
        failure.code.as_str(),
        "build_failed" | "clone_failed" | "build_detection_failed"
    )
}

/// The error for an outcome (`None` on success).
pub fn outcome_error(app: &str, outcome: &Outcome) -> Option<CliError> {
    Some(match outcome {
        Outcome::Succeeded => return None,
        Outcome::Failed(Some(failure), _) => {
            let code = if is_build_failure(failure) {
                code::BUILD_FAILED
            } else {
                code::DEPLOY_FAILED
            };
            CliError::coded(
                code,
                exit::FAILED,
                format!("Deployment failed: {}", failure.title),
            )
            .with_hint(failure.hint.clone())
        }
        Outcome::Failed(None, Some(error)) => CliError::coded(
            code::DEPLOY_FAILED,
            exit::FAILED,
            format!("Deployment failed: {error}"),
        ),
        Outcome::Failed(None, None) => {
            CliError::coded(code::DEPLOY_FAILED, exit::FAILED, "Deployment failed.")
        }
        Outcome::Stopped(status) => CliError::coded(
            if status == "cancelled" {
                local_code::DEPLOY_CANCELLED
            } else {
                local_code::DEPLOY_SUPERSEDED
            },
            exit::CONFLICT,
            format!("Deployment {status}: a newer deploy replaced it or it was cancelled."),
        ),
        Outcome::Unknown => CliError::coded(
            local_code::DEPLOY_OUTCOME_UNKNOWN,
            exit::GENERIC,
            "Lost track of the deployment before it finished.",
        )
        .with_hint(format!("Check it with: tond apps deployments {app}")),
    })
}

/// Report the outcome (human line or the final `result` event) and turn it
/// into the command's result (Err → non-zero exit).
pub fn finish(out: &Output, app: &str, deployment_id: &str, outcome: Outcome) -> Result<()> {
    let error = outcome_error(app, &outcome);
    let failure = match &outcome {
        Outcome::Failed(Some(f), _) => serde_json::to_value(f)?,
        _ => Value::Null,
    };
    out.event(&json!({
        "event": "result",
        "app": app,
        "deployment_id": deployment_id,
        "status": outcome.status(),
        "exit_code": error.as_ref().map(|e| e.exit).unwrap_or(exit::OK),
        "error": error.as_ref().map(|e| json!({"code": e.code, "message": e.message, "hint": e.hint})),
        "failure": failure,
    }))?;
    match error {
        None => {
            outln!(out, "✓ {app} is live.");
            Ok(())
        }
        Some(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error;

    fn record(phase: DeployPhase, status: PhaseStatus) -> PhaseRecord {
        PhaseRecord {
            phase,
            status,
            label: None,
            started_at: Some("2026-09-25T10:00:00Z".into()),
            finished_at: Some("2026-09-25T10:01:04Z".into()),
        }
    }

    fn failure(phase: Option<DeployPhase>, code: &str) -> Outcome {
        Outcome::Failed(
            Some(DeployFailure {
                phase,
                code: code.into(),
                title: "It broke".into(),
                hint: "Check the log.".into(),
                detail: None,
                excerpt: vec![],
            }),
            None,
        )
    }

    fn exit_and_code(outcome: Outcome) -> (u8, Option<String>) {
        let (out, _, _) = Output::capture(false, false, "");
        match finish(&out, "web", "dep-1", outcome) {
            Ok(()) => (0, None),
            Err(e) => {
                let e = error::describe(&e);
                (e.exit, e.code)
            }
        }
    }

    #[test]
    fn phase_changes_print_once() {
        let mut seen = HashMap::new();
        let first = phase_updates(
            &mut seen,
            &[
                record(DeployPhase::Clone, PhaseStatus::Succeeded),
                record(DeployPhase::Build, PhaseStatus::Running),
                record(DeployPhase::Start, PhaseStatus::Pending),
            ],
        );
        assert_eq!(first, vec!["✓ Fetch source (1 min 4 s)", "▸ Build"]);
        let again = phase_updates(
            &mut seen,
            &[
                record(DeployPhase::Clone, PhaseStatus::Succeeded),
                record(DeployPhase::Build, PhaseStatus::Failed),
            ],
        );
        assert_eq!(again, vec!["✗ Build"]);
    }

    #[test]
    fn outcomes_come_from_the_status() {
        assert_eq!(outcome_of(&json!({"status": "running"})), None);
        assert_eq!(
            outcome_of(&json!({"status": "succeeded"})),
            Some(Outcome::Succeeded)
        );
        let failed = outcome_of(&json!({
            "status": "failed",
            "failure": {"code": "build_failed", "title": "The build failed (exit code 1)", "hint": "Check the build log."}
        }));
        let Some(Outcome::Failed(Some(f), _)) = failed.clone() else {
            panic!("expected a classified failure, got {failed:?}");
        };
        assert_eq!(f.code, "build_failed");
        let (out, _, _) = Output::capture(false, false, "");
        let err = finish(&out, "web", "dep-1", failed.unwrap_or(Outcome::Unknown))
            .err()
            .map(|e| error::describe(&e))
            .unwrap_or_else(|| panic!("expected a failure"));
        assert!(err.message.contains("The build failed"));
        assert_eq!(err.hint.as_deref(), Some("Check the build log."));
    }

    #[test]
    fn outcomes_map_to_exit_codes() {
        assert_eq!(exit_and_code(Outcome::Succeeded), (0, None));
        assert_eq!(
            exit_and_code(failure(Some(DeployPhase::Build), "build_failed")),
            (6, Some("BUILD_FAILED".into()))
        );
        assert_eq!(
            exit_and_code(failure(None, "clone_failed")),
            (6, Some("BUILD_FAILED".into()))
        );
        assert_eq!(
            exit_and_code(failure(Some(DeployPhase::Health), "health_check_failed")),
            (6, Some("DEPLOY_FAILED".into()))
        );
        assert_eq!(
            exit_and_code(Outcome::Failed(None, Some("oom".into()))),
            (6, Some("DEPLOY_FAILED".into()))
        );
        assert_eq!(
            exit_and_code(Outcome::Stopped("superseded".into())),
            (5, Some("DEPLOY_SUPERSEDED".into()))
        );
        assert_eq!(
            exit_and_code(Outcome::Stopped("cancelled".into())),
            (5, Some("DEPLOY_CANCELLED".into()))
        );
        assert_eq!(
            exit_and_code(Outcome::Unknown),
            (1, Some("DEPLOY_OUTCOME_UNKNOWN".into()))
        );
    }

    #[test]
    fn json_mode_ends_with_a_result_event() {
        let (out, stdout, _) = Output::capture(true, false, "");
        let _ = finish(
            &out,
            "web",
            "dep-1",
            failure(Some(DeployPhase::Build), "build_failed"),
        );
        let event: Value = serde_json::from_str(stdout.text().trim()).unwrap_or_default();
        assert_eq!(
            event,
            json!({
                "event": "result",
                "app": "web",
                "deployment_id": "dep-1",
                "status": "failed",
                "exit_code": 6,
                "error": {"code": "BUILD_FAILED", "message": "Deployment failed: It broke", "hint": "Check the log."},
                "failure": {"phase": "build", "code": "build_failed", "title": "It broke", "hint": "Check the log."}
            })
        );
    }

    #[test]
    fn durations() {
        assert_eq!(format_secs(4), "4 s");
        assert_eq!(format_secs(64), "1 min 4 s");
        assert_eq!(format_secs(120), "2 min");
    }
}
