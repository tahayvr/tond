// log_bus.rs — in-process fan-out of deploy events
//
// deploy::log_deploy() inserts every deploy log line into `deploy_logs` and
// then publishes it here; deploy::progress publishes a `Deployment` event
// whenever a deployment row changes (phase started/finished, commit recorded,
// result). Live WebSockets (handlers/logs.rs, the deployment stream in
// handlers/deployments.rs) subscribe instead of polling the tables, so an
// open logs tab or deploy view costs nothing while no deploy is running.
//
// The bus is a process-wide tokio broadcast channel. Publishing with no
// subscribers is a cheap no-op. A subscriber that falls more than CAPACITY
// events behind gets `RecvError::Lagged` and is told how many it missed
// (they remain in the database). Events are published in the order they
// happen, so a phase change and the log lines around it arrive in order.

use std::sync::{Arc, LazyLock};

use tokio::sync::broadcast;

/// Events buffered per subscriber before it starts lagging.
const CAPACITY: usize = 1024;

/// One deploy log line, as stored in `deploy_logs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployLogEvent {
    pub id: String,
    pub app_id: String,
    /// The deployment the line belongs to (None for lines outside one, such
    /// as preview builds).
    pub deployment_id: Option<String>,
    pub level: String,
    pub message: String,
    pub created_at: String,
}

/// Something happened on the deploy pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployEvent {
    /// A log line was written.
    Log(DeployLogEvent),
    /// A deployment row changed (phase, commit, status or failure); readers
    /// re-read the row.
    Deployment {
        app_id: String,
        deployment_id: String,
    },
}

static BUS: LazyLock<broadcast::Sender<Arc<DeployEvent>>> =
    LazyLock::new(|| broadcast::channel(CAPACITY).0);

/// Publish a line that has just been written to `deploy_logs`.
pub fn publish(event: DeployLogEvent) {
    // Err only means "no subscribers right now".
    let _ = BUS.send(Arc::new(DeployEvent::Log(event)));
}

/// Publish that a deployment row changed.
pub fn publish_deployment(app_id: &str, deployment_id: &str) {
    let _ = BUS.send(Arc::new(DeployEvent::Deployment {
        app_id: app_id.to_string(),
        deployment_id: deployment_id.to_string(),
    }));
}

/// Subscribe to events published from now on.
pub fn subscribe() -> broadcast::Receiver<Arc<DeployEvent>> {
    BUS.subscribe()
}
