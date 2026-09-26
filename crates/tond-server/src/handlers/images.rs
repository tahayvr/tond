// handlers/images.rs — Docker image inspection for the new-app wizard
//
// POST /api/images/inspect
//   Body: { "image": "nginx:alpine", "registry_credential_id"?: "..." }
//   Pulls the image if it isn't on the server yet (the first deploy needs it
//   anyway, so the pull is not wasted) and reports the ports its config
//   exposes (EXPOSE) plus a PORT environment default, so the wizard can
//   suggest the right container port instead of a blind 8080.

use std::time::Duration;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use tond_common::{ApiResponse, TondError};

use crate::{
    auth::{Authz, RateLimit, scope},
    deploy,
    error::AppError,
    state::AppState,
};

/// A pull for inspection gives up after this long.
const PULL_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Deserialize)]
pub struct InspectImageRequest {
    pub image: String,
    pub registry_credential_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct InspectImageResponse {
    pub image: String,
    /// TCP ports from the image's EXPOSE instructions, ascending
    pub exposed_ports: Vec<u16>,
    /// A `PORT=<n>` default in the image's environment
    pub env_port: Option<u16>,
    /// The port the wizard should suggest, if the image says
    pub suggested_port: Option<u16>,
    /// The image had to be pulled for this
    pub pulled: bool,
}

/// TCP ports from Docker's `ExposedPorts` keys ("80/tcp", "53/udp", "8080").
pub fn tcp_ports(exposed: &[String]) -> Vec<u16> {
    let mut ports: Vec<u16> = exposed
        .iter()
        .filter_map(|key| {
            let (port, proto) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
            (proto == "tcp").then(|| port.parse().ok()).flatten()
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// `PORT=3000` in an image's environment.
pub fn env_port(env: &[String]) -> Option<u16> {
    env.iter()
        .find_map(|kv| kv.strip_prefix("PORT="))
        .and_then(|v| v.trim().parse().ok())
}

/// Which port to suggest: an exposed port that matches PORT, else the only
/// or the most web-like exposed port, else PORT.
pub fn suggest_port(exposed: &[u16], env_port: Option<u16>) -> Option<u16> {
    if let Some(p) = env_port
        && (exposed.is_empty() || exposed.contains(&p))
    {
        return Some(p);
    }
    const WEB: [u16; 6] = [80, 8080, 3000, 8000, 5000, 443];
    if exposed.len() == 1 {
        return exposed.first().copied();
    }
    WEB.iter()
        .find(|p| exposed.contains(p))
        .copied()
        .or_else(|| exposed.first().copied())
}

pub async fn inspect_image(
    _rate: RateLimit,
    _auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Json(req): Json<InspectImageRequest>,
) -> Result<impl IntoResponse, AppError> {
    let image = req.image.trim().to_string();
    if image.is_empty()
        || image.starts_with('-')
        || image.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError(TondError::InvalidInput(
            "Enter an image reference like nginx:alpine or ghcr.io/org/app:1.2".to_string(),
        )));
    }

    let mut pulled = false;
    if state.docker.inspect_image(&image).await.is_err() {
        let credentials = match &req.registry_credential_id {
            Some(id) => deploy::fetch_registry_credentials(&state, id)
                .await
                .map_err(|e| AppError(TondError::Internal(e.to_string())))?,
            None => None,
        };
        tokio::time::timeout(
            PULL_TIMEOUT,
            deploy::pull_image(&state.docker, &image, credentials),
        )
        .await
        .map_err(|_| {
            AppError(TondError::InvalidInput(
                "Pulling the image took too long; set the port by hand".to_string(),
            ))
        })?
        .map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "Couldn't pull {image}: {e}. Check the name and tag; private images need registry credentials."
            )))
        })?;
        pulled = true;
    }

    let info = state
        .docker
        .inspect_image(&image)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    let config = info.config.unwrap_or_default();
    let exposed_ports = tcp_ports(&config.exposed_ports.unwrap_or_default());
    let env_port = env_port(&config.env.unwrap_or_default());
    let suggested_port = suggest_port(&exposed_ports, env_port);

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(InspectImageResponse {
            image,
            exposed_ports,
            env_port,
            suggested_port,
            pulled,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_tcp_ports_and_port_env() {
        assert_eq!(
            tcp_ports(&s(&["8080/tcp", "53/udp", "80", "80/tcp"])),
            vec![80, 8080]
        );
        assert_eq!(env_port(&s(&["PATH=/bin", "PORT=3000"])), Some(3000));
        assert_eq!(env_port(&s(&["PORT=abc"])), None);
    }

    #[test]
    fn suggests_the_port_the_image_means() {
        assert_eq!(suggest_port(&[80], None), Some(80));
        assert_eq!(suggest_port(&[443, 80], None), Some(80));
        assert_eq!(suggest_port(&[9000, 9001], None), Some(9000));
        assert_eq!(suggest_port(&[], Some(3000)), Some(3000));
        assert_eq!(suggest_port(&[8080, 3000], Some(3000)), Some(3000));
        assert_eq!(suggest_port(&[80], Some(3000)), Some(80));
        assert_eq!(suggest_port(&[], None), None);
    }
}
