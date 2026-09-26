// projects.rs — Project entity and all project request/response types

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Project (app group) ──

/// A project groups related apps under a single logical unit and provides
/// shared environment variables injected into every member app at deploy time.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Shared environment variables: a JSON object, stored encrypted by the
    /// server (`{}` when empty). Never returned: responses carry masked keys.
    #[serde(skip_serializing)]
    pub env_vars: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Project {
    pub fn new(name: String, description: Option<String>) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            description,
            env_vars: "{}".to_string(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Shared env var keys with masked values (values are secrets).
    #[serde(default)]
    pub env: Vec<crate::EnvVarEntry>,
    pub created_at: String,
    pub updated_at: String,
    /// Number of active (non-expired) project previews for this project.
    /// Computed via a LEFT JOIN at query time; defaults to 0.
    pub active_preview_count: i64,
}

impl ProjectResponse {
    /// Build a response from a project and its (decrypted) shared env.
    pub fn new(
        p: Project,
        env: &std::collections::BTreeMap<String, String>,
        active_preview_count: i64,
    ) -> Self {
        Self {
            id: p.id,
            name: p.name,
            description: p.description,
            env: crate::masked_env_entries(env),
            created_at: p.created_at,
            updated_at: p.updated_at,
            active_preview_count,
        }
    }
}

/// POST /api/projects
#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
    /// Optional initial shared environment variables
    pub env_vars: Option<serde_json::Value>,
}

/// PUT /api/projects/{id}
#[derive(Debug, Deserialize)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    /// Replace the full shared env_vars JSON object
    pub env_vars: Option<serde_json::Value>,
}

/// PUT /api/apps/{id}/project — assign or remove an app's project
#[derive(Debug, Deserialize)]
pub struct AssignProjectRequest {
    /// Project ID to assign. null = remove from any project.
    pub project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_new_sets_empty_env_vars_object() {
        let project = Project::new("platform".to_string(), Some("Shared services".to_string()));

        assert_eq!(
            project.name, "platform",
            "Project::new should preserve the project name"
        );
        assert_eq!(
            project.env_vars, "{}",
            "Project::new should initialize shared env vars as an empty JSON object"
        );
    }

    #[test]
    fn project_response_masks_shared_env_values() {
        let project = Project::new("platform".to_string(), None);
        let env =
            std::collections::BTreeMap::from([("ENV".to_string(), "prod-secret".to_string())]);

        let response = ProjectResponse::new(project, &env, 2);
        let json = serde_json::to_string(&response).unwrap_or_default();

        assert!(
            !json.contains("prod-secret"),
            "values must be masked: {json}"
        );
        assert_eq!(response.env[0].key, "ENV");
        assert_eq!(response.env[0].masked, crate::ENV_VALUE_MASK);
        assert_eq!(response.active_preview_count, 2);
    }
}
