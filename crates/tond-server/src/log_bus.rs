// log_bus.rs — in-process fan-out of deploy log lines
//
// deploy::log_deploy() inserts every deploy log line into `deploy_logs` and
// then publishes it here. Live log WebSockets (handlers/logs.rs) subscribe
// instead of polling the table every second, so an open logs tab costs
// nothing while no deploy is running.
//
// The bus is a process-wide tokio broadcast channel. Publishing with no
// subscribers is a cheap no-op. A subscriber that falls more than CAPACITY
// lines behind gets `RecvError::Lagged` and is told how many lines it missed
// (they remain in the database).

use std::sync::{Arc, LazyLock};

use tokio::sync::broadcast;

/// Lines buffered per subscriber before it starts lagging.
const CAPACITY: usize = 1024;

/// One deploy log line, as stored in `deploy_logs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployLogEvent {
    pub id: String,
    pub app_id: String,
    pub level: String,
    pub message: String,
    pub created_at: String,
}

static BUS: LazyLock<broadcast::Sender<Arc<DeployLogEvent>>> =
    LazyLock::new(|| broadcast::channel(CAPACITY).0);

/// Publish a line that has just been written to `deploy_logs`.
pub fn publish(event: DeployLogEvent) {
    // Err only means "no subscribers right now".
    let _ = BUS.send(Arc::new(event));
}

/// Subscribe to lines published from now on.
pub fn subscribe() -> broadcast::Receiver<Arc<DeployLogEvent>> {
    BUS.subscribe()
}
