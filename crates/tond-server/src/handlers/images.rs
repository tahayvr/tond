// handlers/images.rs — Docker image inspection for the new-app wizard
//
// POST /api/v1/images/inspect
//   Body: { "image": "nginx:alpine", "registry_credential_id"?: "..." }
//   Pulls the image if it isn't on the server yet (the first deploy needs it
//   anyway, so the pull is not wasted) and reports the ports its config
//   exposes (EXPOSE) plus a PORT environment default, so the wizard can
//   suggest the right container port instead of a blind 8080.
//
//   A failed pull is a soft failure (200 with `unavailable` set): a private
//   image can't be read before registry credentials are chosen, and the
//   wizard then asks for the port by hand or reads it again with them.

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
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct InspectImageRequest {
    pub image: String,
    pub registry_credential_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
    /// Set when the image couldn't be read (a soft failure: the port is then
    /// entered by hand). Says why and what to do.
    pub unavailable: Option<String>,
    /// The registry refused access (or claims no such image): the image may be
    /// private and need registry credentials.
    pub needs_credentials: bool,
}

/// A soft failure's hint: whether credentials may help, and what to say.
pub fn pull_failure_hint(image: &str, error: &str, had_credentials: bool) -> (bool, String) {
    let lower = error.to_ascii_lowercase();
    let denied = [
        "unauthorized",
        "denied",
        "authentication required",
        "401",
        "403",
        "not found",
        "manifest unknown",
        "404",
        "does not exist",
    ]
    .iter()
    .any(|m| lower.contains(m));
    if denied && !had_credentials {
        (
            true,
            format!(
                "Tond couldn't read {image}. If it's private, choose registry credentials on the Environment step and Tond reads its port then; otherwise check the name and tag."
            ),
        )
    } else if denied {
        (
            true,
            format!(
                "The registry refused {image} with the chosen credentials, or it doesn't exist. Check the image name, tag and credentials."
            ),
        )
    } else {
        (
            false,
            format!(
                "Tond couldn't pull {image} to read its port ({error}). Enter the port by hand."
            ),
        )
    }
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
        let had_credentials = credentials.is_some();
        let outcome = tokio::time::timeout(
            PULL_TIMEOUT,
            deploy::pull_image(&state.docker, &image, credentials),
        )
        .await;
        let error = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some("the pull took too long".to_string()),
        };
        // Fail softly: the wizard shows the hint and the port is typed by hand
        // (or read again once credentials are chosen).
        if let Some(error) = error {
            let (needs_credentials, hint) = pull_failure_hint(&image, &error, had_credentials);
            return Ok((
                StatusCode::OK,
                Json(ApiResponse::ok(InspectImageResponse {
                    image,
                    exposed_ports: Vec::new(),
                    env_port: None,
                    suggested_port: None,
                    pulled: false,
                    unavailable: Some(hint),
                    needs_credentials,
                })),
            ));
        }
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
            unavailable: None,
            needs_credentials: false,
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
    fn pull_failures_hint_at_credentials() {
        let (creds, hint) = pull_failure_hint(
            "ghcr.io/acme/app:1",
            "Docker responded with status code 401: unauthorized",
            false,
        );
        assert!(creds && hint.contains("registry credentials on the Environment step"));
        let (creds, hint) = pull_failure_hint("x", "manifest unknown", true);
        assert!(creds && hint.contains("chosen credentials"));
        let (creds, hint) = pull_failure_hint("x", "the pull took too long", false);
        assert!(!creds && hint.contains("by hand"));
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
