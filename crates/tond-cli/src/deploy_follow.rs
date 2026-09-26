// deploy_follow.rs — `tond apps deploy` follows the deployment it started
//
// Reads the deployment stream (GET /api/apps/{id}/deployments/{dep}/stream):
// a snapshot with the row and its log so far, then `log` lines and
// `deployment` updates until the server closes the socket shortly after the
// deployment finishes. Phase changes print as
//
//   ▸ Build
//     │ Step 3/7 : RUN npm ci
//   ✓ Build (1 min 4 s)
//
// and the command ends with the outcome: exit 0 on success, an error (non-zero
// exit) with the likely cause and a hint on failure.

use std::collections::HashMap;

use anyhow::{Result, bail};
use chrono::DateTime;
use futures::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use tond_common::{DeployFailure, DeployPhase, PhaseRecord, PhaseStatus};

use crate::client::TondClient;

/// Where the deployment ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed(Option<DeployFailure>, Option<String>),
    /// Superseded or cancelled.
    Stopped(String),
    /// The stream ended before the deployment finished.
    Unknown,
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

/// Lines to print for the phases that changed since `seen`, which is updated.
pub fn phase_updates(
    seen: &mut HashMap<DeployPhase, PhaseStatus>,
    phases: &[PhaseRecord],
) -> Vec<String> {
    let mut lines = Vec::new();
    for record in phases {
        let before = seen.insert(record.phase, record.status);
        if before == Some(record.status) {
            continue;
        }
        let name = phase_name(record);
        match record.status {
            PhaseStatus::Running => lines.push(format!("▸ {name}")),
            PhaseStatus::Succeeded => {
                let took =
                    seconds_between(record.started_at.as_deref(), record.finished_at.as_deref())
                        .map(|s| format!(" ({})", format_secs(s)))
                        .unwrap_or_default();
                lines.push(format!("✓ {name}{took}"));
            }
            PhaseStatus::Failed => lines.push(format!("✗ {name}")),
            PhaseStatus::Pending | PhaseStatus::Skipped => {}
        }
    }
    lines
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

fn print_log(line: &Value) {
    let message = line.get("message").and_then(Value::as_str).unwrap_or("");
    for part in message.lines() {
        println!("  │ {part}");
    }
}

/// Follow `deployment_id` of `app` until it finishes; print progress.
pub async fn follow(client: &TondClient, app: &str, deployment_id: &str) -> Result<Outcome> {
    let mut stream = client
        .ws_messages(&format!(
            "/api/apps/{app}/deployments/{deployment_id}/stream"
        ))
        .await?;
    let mut seen: HashMap<DeployPhase, PhaseStatus> = HashMap::new();
    let mut outcome = Outcome::Unknown;

    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(frame)) => {
                if let Some(f) = frame
                    && u16::from(f.code) >= 4000
                {
                    bail!("{}", f.reason);
                }
                break;
            }
            Ok(_) => continue,
            Err(e) => bail!("Lost the deployment stream: {e}"),
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
                    print_log(line);
                }
                for line in phase_updates(&mut seen, &phases_of(&deployment)) {
                    println!("{line}");
                }
                if let Some(o) = outcome_of(&deployment) {
                    outcome = o;
                }
            }
            Some("log") => print_log(&frame),
            Some("deployment") => {
                let deployment = frame.get("deployment").cloned().unwrap_or_default();
                for line in phase_updates(&mut seen, &phases_of(&deployment)) {
                    println!("{line}");
                }
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

/// Turn the outcome into the command's result (Err → non-zero exit).
pub fn finish(app: &str, outcome: Outcome) -> Result<()> {
    match outcome {
        Outcome::Succeeded => {
            println!("✓ {app} is live.");
            Ok(())
        }
        Outcome::Failed(Some(failure), _) => {
            bail!("Deployment failed: {}\n  {}", failure.title, failure.hint)
        }
        Outcome::Failed(None, Some(error)) => bail!("Deployment failed: {error}"),
        Outcome::Failed(None, None) => bail!("Deployment failed."),
        Outcome::Stopped(status) => {
            bail!("Deployment {status}: a newer deploy replaced it or it was cancelled.")
        }
        Outcome::Unknown => bail!(
            "Lost track of the deployment before it finished. Check it with: tond apps deployments {app}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record(phase: DeployPhase, status: PhaseStatus) -> PhaseRecord {
        PhaseRecord {
            phase,
            status,
            label: None,
            started_at: Some("2026-09-25T10:00:00Z".into()),
            finished_at: Some("2026-09-25T10:01:04Z".into()),
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
        let err = finish("web", failed.unwrap_or(Outcome::Unknown))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("The build failed") && err.contains("Check the build log."));
        assert!(finish("web", Outcome::Succeeded).is_ok());
        assert!(finish("web", Outcome::Stopped("superseded".into())).is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(format_secs(4), "4 s");
        assert_eq!(format_secs(64), "1 min 4 s");
        assert_eq!(format_secs(120), "2 min");
    }
}
