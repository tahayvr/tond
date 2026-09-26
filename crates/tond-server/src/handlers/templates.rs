use axum::{Json, http::StatusCode, response::IntoResponse};
use tond_common::{ApiResponse, TemplateCatalogEntry, TondError, load_template_manifests};

use crate::{
    auth::{Authz, RateLimit, scope},
    error::AppError,
};

pub async fn list_templates(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
) -> Result<impl IntoResponse, AppError> {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates");
    let entries = load_template_manifests(&template_dir).map_err(|err| {
        AppError(TondError::Internal(format!(
            "failed to load template catalog: {err}"
        )))
    })?;
    let templates: Vec<TemplateCatalogEntry> = entries
        .into_iter()
        .map(|(_, manifest)| TemplateCatalogEntry::from(manifest))
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(templates))))
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    #[tokio::test]
    async fn list_templates_returns_manifest_catalog_entries() {
        let response = list_templates(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
        )
        .await
        .unwrap_or_else(|_| panic!("expected list_templates to succeed"))
        .into_response();
        let body = response_json(response).await;
        let templates = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected templates payload to be an array"));
        let plausible = templates
            .iter()
            .find(|template| template["id"] == "plausible")
            .unwrap_or_else(|| panic!("expected plausible template to be present in catalog"));

        assert_eq!(
            plausible["project"]["name"],
            serde_json::Value::String("plausible".to_string()),
            "list_templates should expose project-scoped template metadata"
        );
        assert_eq!(
            plausible["dependencies"][0]["name"],
            serde_json::Value::String("plausible-db".to_string()),
            "list_templates should include declared dependencies so dashboard and CLI share the same manifest data"
        );
        assert_eq!(
            plausible["required_env"][2]["value"],
            serde_json::Value::String("${db.plausible-db.internal_connection_string}".to_string()),
            "list_templates should expose interpolated env definitions for template-aware creation flows"
        );
    }
}
