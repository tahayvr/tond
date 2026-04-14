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
    /// Shared environment variables as a JSON object (merged on top of each
    /// member app's own env_vars at deploy time; app vars take precedence).
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
    pub env_vars: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    /// Number of active (non-expired) project previews for this project.
    /// Computed via a LEFT JOIN at query time; defaults to 0.
    pub active_preview_count: i64,
}

impl From<Project> for ProjectResponse {
    fn from(p: Project) -> Self {
        let env_vars: serde_json::Value = serde_json::from_str(&p.env_vars)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        Self {
            id: p.id,
            name: p.name,
            description: p.description,
            env_vars,
            created_at: p.created_at,
            updated_at: p.updated_at,
            active_preview_count: 0,
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
    fn project_response_from_project_parses_valid_env_json() {
        let project = Project {
            id: "project-1".to_string(),
            name: "platform".to_string(),
            description: Some("Shared services".to_string()),
            env_vars: serde_json::json!({ "ENV": "prod" }).to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let response = ProjectResponse::from(project);

        assert_eq!(
            response.env_vars,
            serde_json::json!({ "ENV": "prod" }),
            "ProjectResponse should deserialize the stored env_vars JSON"
        );
        assert_eq!(
            response.active_preview_count, 0,
            "ProjectResponse::from should default active_preview_count to 0"
        );
    }

    #[test]
    fn project_response_from_project_falls_back_to_empty_object_for_invalid_env_json() {
        let project = Project {
            id: "project-1".to_string(),
            name: "platform".to_string(),
            description: None,
            env_vars: "not json".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let response = ProjectResponse::from(project);

        assert_eq!(
            response.env_vars,
            serde_json::json!({}),
            "invalid stored env_vars JSON should fall back to an empty object"
        );
    }
}
