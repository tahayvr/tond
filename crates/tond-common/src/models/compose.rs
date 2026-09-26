// compose.rs — Docker Compose apps: services, per-service routing, per-deployment images

use serde::{Deserialize, Serialize};

/// A service of a compose app as the file declares it (from
/// `POST /api/compose/validate` and compose saves), before anything runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeServiceSpec {
    pub name: String,
    /// Image ref from the file (None for build-only services)
    #[serde(default)]
    pub image: Option<String>,
    /// The service has a `build:` section (built by Tond on deploy)
    #[serde(default)]
    pub builds: bool,
    /// Container ports declared under `ports:` and `expose:`
    #[serde(default)]
    pub ports: Vec<u16>,
    /// The service defines a healthcheck
    #[serde(default)]
    pub has_healthcheck: bool,
}

/// `GET /api/apps/{id}/services` item: one compose service with what Docker
/// reports about it now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceResponse {
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub builds: bool,
    /// Container ports declared in the file
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub has_healthcheck: bool,
    /// Container port Tond publishes for routing (None = not published)
    #[serde(default)]
    pub published_port: Option<u16>,
    /// Loopback host port the published port is bound to
    #[serde(default)]
    pub host_port: Option<u16>,
    /// The published port changed since the last deploy: redeploy to apply
    #[serde(default)]
    pub redeploy_required: bool,
    /// Domains routed to this service
    #[serde(default)]
    pub domains: Vec<String>,
    /// "pending" | "running" | "restarting" | "paused" | "created" | "exited"
    /// | "dead" | "missing"
    pub status: String,
    /// "healthy" | "unhealthy" | "starting"; None without a healthcheck
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub container_id: Option<String>,
    /// Pinned local image the container runs (`tond-<app>/<service>:<id>`)
    #[serde(default)]
    pub running_image: Option<String>,
    /// Image id of the running image (sha256:...)
    #[serde(default)]
    pub image_digest: Option<String>,
    /// Latest CPU use in 1/100 % of one core (None before the first sample)
    #[serde(default)]
    pub cpu: Option<i64>,
    /// Latest memory use in bytes
    #[serde(default)]
    pub mem: Option<i64>,
}

/// `PUT /api/apps/{id}/services/{service}`: publish a container port and
/// route domains to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateComposeServiceRequest {
    /// Container port to publish on a loopback host port (None = none)
    #[serde(default)]
    pub published_port: Option<u16>,
    /// Domains to route to the published port (replaces the current list)
    #[serde(default)]
    pub domains: Vec<String>,
}

/// `PUT /api/apps/{id}/compose`: replace the compose file (inline) or the
/// repository location of a git-sourced compose app.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateComposeRequest {
    #[serde(default)]
    pub compose_content: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub compose_path: Option<String>,
}

/// One service of a compose deployment: how its image was produced and the
/// pinned tag rollback restores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DeploymentService {
    pub service: String,
    /// "build" | "pull" | "reuse"
    pub source: String,
    #[serde(default)]
    pub source_image: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    /// "pending" | "building" | "pulling" | "ready" | "starting" | "healthy"
    /// | "running" | "failed"
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_update_defaults_to_nothing_published() {
        let req: UpdateComposeServiceRequest =
            serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(req.published_port, None);
        assert!(req.domains.is_empty());
    }
}
