use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TemplateManifestError {
    #[error("failed to read template manifest {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse template manifest {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("template manifest {path} is invalid: {message}")]
    Invalid { path: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub maintainers: Vec<String>,
    pub project: Option<TemplateProject>,
    pub source: TemplateSource,
    pub deploy: TemplateDeploy,
    #[serde(default)]
    pub required_env: Vec<TemplateEnvVar>,
    #[serde(default)]
    pub optional_env: Vec<TemplateEnvVar>,
    #[serde(default)]
    pub dependencies: Vec<TemplateDependency>,
    #[serde(default)]
    pub recommended_domains: Vec<String>,
    #[serde(default)]
    pub post_deploy_checks: Vec<String>,
    pub setup_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateCatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub maintainers: Vec<String>,
    pub project: Option<TemplateProject>,
    pub source: TemplateSource,
    pub deploy: TemplateDeploy,
    pub required_env: Vec<TemplateEnvVar>,
    pub optional_env: Vec<TemplateEnvVar>,
    pub dependencies: Vec<TemplateDependency>,
    pub recommended_domains: Vec<String>,
    pub post_deploy_checks: Vec<String>,
    pub setup_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateSource {
    pub kind: String,
    pub url: String,
    #[serde(alias = "reference")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDeploy {
    pub method: String,
    pub default_port: u16,
    pub health_check_path: Option<String>,
    #[serde(default)]
    pub volumes: Vec<TemplateVolume>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateProject {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateVolume {
    pub container_path: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateEnvVar {
    pub key: String,
    pub description: String,
    pub required: Option<bool>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDependency {
    pub name: Option<String>,
    pub kind: String,
    pub engine: Option<String>,
    pub required: Option<bool>,
}

impl TemplateManifest {
    pub fn from_toml_str(path: &Path, contents: &str) -> Result<Self, TemplateManifestError> {
        let manifest: TemplateManifest =
            toml::from_str(contents).map_err(|source| TemplateManifestError::Parse {
                path: path.display().to_string(),
                source,
            })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self, TemplateManifestError> {
        let contents = fs::read_to_string(path).map_err(|source| TemplateManifestError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml_str(path, &contents)
    }

    fn validate(&self, path: &Path) -> Result<(), TemplateManifestError> {
        if self.id.trim().is_empty() {
            return Err(invalid(path, "id must not be empty"));
        }
        if self.name.trim().is_empty() {
            return Err(invalid(path, "name must not be empty"));
        }
        if self.description.trim().is_empty() {
            return Err(invalid(path, "description must not be empty"));
        }
        if self.category.trim().is_empty() {
            return Err(invalid(path, "category must not be empty"));
        }
        if self.source.url.trim().is_empty() {
            return Err(invalid(path, "source.url must not be empty"));
        }
        if let Some(project) = &self.project
            && project.name.trim().is_empty()
        {
            return Err(invalid(
                path,
                "project.name must not be empty when provided",
            ));
        }
        if let Some(source_ref) = &self.source.r#ref
            && source_ref.trim().is_empty()
        {
            return Err(invalid(path, "source.ref must not be empty when provided"));
        }
        if self.deploy.default_port == 0 {
            return Err(invalid(path, "deploy.default_port must be greater than 0"));
        }
        for maintainer in &self.maintainers {
            if maintainer.trim().is_empty() {
                return Err(invalid(path, "template maintainers must not be empty"));
            }
        }

        let source_kind = self.source.kind.as_str();
        let deploy_method = self.deploy.method.as_str();

        let source_is_valid = matches!(source_kind, "git" | "image");
        if !source_is_valid {
            return Err(invalid(path, "source.kind must be one of: git, image"));
        }

        let deploy_is_valid = matches!(deploy_method, "tbuild" | "dockerfile" | "image");
        if !deploy_is_valid {
            return Err(invalid(
                path,
                "deploy.method must be one of: tbuild, dockerfile, image",
            ));
        }

        match (source_kind, deploy_method) {
            ("git", "tbuild" | "dockerfile") => {}
            ("image", "image") => {}
            ("git", "image") => {
                return Err(invalid(
                    path,
                    "git templates must deploy via tbuild or dockerfile",
                ));
            }
            ("image", _) => {
                return Err(invalid(path, "image templates must deploy via image"));
            }
            _ => unreachable!("validated source kinds should cover all branches"),
        }

        for env in self.required_env.iter().chain(self.optional_env.iter()) {
            if env.key.trim().is_empty() {
                return Err(invalid(path, "template env vars must have a non-empty key"));
            }
            if env.description.trim().is_empty() {
                return Err(invalid(
                    path,
                    "template env vars must have a non-empty description",
                ));
            }
            if let Some(value) = &env.value
                && value.trim().is_empty()
            {
                return Err(invalid(
                    path,
                    "template env vars with value must not be empty",
                ));
            }
        }

        for volume in &self.deploy.volumes {
            if !volume.container_path.starts_with('/') {
                return Err(invalid(
                    path,
                    "template volumes must use absolute container paths",
                ));
            }
            if volume.description.trim().is_empty() {
                return Err(invalid(
                    path,
                    "template volumes must have a non-empty description",
                ));
            }
        }

        for domain in &self.recommended_domains {
            if domain.trim().is_empty() {
                return Err(invalid(
                    path,
                    "recommended_domains must not contain empty values",
                ));
            }
        }

        for dependency in &self.dependencies {
            if let Some(name) = &dependency.name
                && name.trim().is_empty()
            {
                return Err(invalid(
                    path,
                    "template dependency names must not be empty when provided",
                ));
            }
        }

        for check in &self.post_deploy_checks {
            if check.trim().is_empty() {
                return Err(invalid(
                    path,
                    "post_deploy_checks must not contain empty values",
                ));
            }
        }

        Ok(())
    }
}

impl From<TemplateManifest> for TemplateCatalogEntry {
    fn from(manifest: TemplateManifest) -> Self {
        Self {
            id: manifest.id,
            name: manifest.name,
            description: manifest.description,
            category: manifest.category,
            tags: manifest.tags,
            maintainers: manifest.maintainers,
            project: manifest.project,
            source: manifest.source,
            deploy: manifest.deploy,
            required_env: manifest.required_env,
            optional_env: manifest.optional_env,
            dependencies: manifest.dependencies,
            recommended_domains: manifest.recommended_domains,
            post_deploy_checks: manifest.post_deploy_checks,
            setup_notes: manifest.setup_notes,
        }
    }
}

pub fn load_template_manifests(
    dir: &Path,
) -> Result<Vec<(PathBuf, TemplateManifest)>, TemplateManifestError> {
    let mut manifests = Vec::new();
    let entries = fs::read_dir(dir).map_err(|source| TemplateManifestError::Read {
        path: dir.display().to_string(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| TemplateManifestError::Read {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let manifest = TemplateManifest::from_file(&path)?;
        manifests.push((path, manifest));
    }

    manifests.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(manifests)
}

fn invalid(path: &Path, message: impl Into<String>) -> TemplateManifestError {
    TemplateManifestError::Invalid {
        path: path.display().to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    fn templates_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
    }

    fn temp_dir() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"))
    }

    #[test]
    fn template_catalog_manifests_are_well_formed() {
        let manifests = load_template_manifests(&templates_dir())
            .unwrap_or_else(|err| panic!("expected template manifests to load: {err}"));

        assert!(
            !manifests.is_empty(),
            "template catalog should contain at least one manifest"
        );
    }

    #[test]
    fn validate_rejects_invalid_source_and_deploy_combinations() {
        let path = Path::new("invalid.toml");
        let contents = r#"
id = "bad"
name = "Bad"
description = "Invalid template"
category = "test"

[source]
kind = "git"
url = "https://example.com/repo.git"

[deploy]
method = "image"
default_port = 8080
"#;

        let err = TemplateManifest::from_toml_str(path, contents)
            .expect_err("git template using image deploy method should be rejected");

        match err {
            TemplateManifestError::Invalid { message, .. } => assert_eq!(
                message, "git templates must deploy via tbuild or dockerfile",
                "validation should explain the source/deploy mismatch clearly"
            ),
            other => panic!("expected invalid manifest error, got {other}"),
        }
    }

    #[test]
    fn load_template_manifests_ignores_non_toml_files() {
        let dir = temp_dir();
        fs::write(
            dir.path().join("demo.toml"),
            r#"
id = "demo"
name = "Demo"
description = "Demo template"
category = "test"

[source]
kind = "image"
url = "ghcr.io/example/demo"

[deploy]
method = "image"
default_port = 8080
"#,
        )
        .unwrap_or_else(|err| panic!("failed to write demo template: {err}"));
        fs::write(dir.path().join("README.md"), "ignored")
            .unwrap_or_else(|err| panic!("failed to write ignored file: {err}"));

        let manifests = load_template_manifests(dir.path())
            .unwrap_or_else(|err| panic!("expected manifest loader to succeed: {err}"));

        assert_eq!(
            manifests.len(),
            1,
            "loader should ignore non-TOML files in the templates directory"
        );
        assert_eq!(
            manifests[0].1.id, "demo",
            "loader should return the parsed template manifest"
        );
    }

    #[test]
    fn manifest_accepts_source_ref_and_legacy_reference_alias() {
        let path = Path::new("template.toml");
        let manifest = TemplateManifest::from_toml_str(
            path,
            r#"
id = "demo"
name = "Demo"
description = "Demo template"
category = "test"

[source]
kind = "git"
url = "https://example.com/repo.git"
ref = "refs/tags/v1.2.3"

[deploy]
method = "tbuild"
default_port = 8080
"#,
        )
        .unwrap_or_else(|err| panic!("expected manifest with source.ref to parse: {err}"));

        assert_eq!(
            manifest.source.r#ref.as_deref(),
            Some("refs/tags/v1.2.3"),
            "manifest should preserve the explicit source.ref value"
        );

        let aliased = TemplateManifest::from_toml_str(
            path,
            r#"
id = "demo"
name = "Demo"
description = "Demo template"
category = "test"

[source]
kind = "image"
url = "ghcr.io/example/demo"
reference = "2026.04"

[deploy]
method = "image"
default_port = 8080
"#,
        )
        .unwrap_or_else(|err| panic!("expected manifest with reference alias to parse: {err}"));

        assert_eq!(
            aliased.source.r#ref.as_deref(),
            Some("2026.04"),
            "legacy reference field should deserialize into source.ref"
        );
    }

    #[test]
    fn manifest_supports_richer_metadata_fields() {
        let manifest = TemplateManifest::from_toml_str(
            Path::new("rich.toml"),
            r#"
id = "demo"
name = "Demo"
description = "Demo template"
category = "analytics"
maintainers = ["tond", "community"]
recommended_domains = ["stats.example.com"]
post_deploy_checks = ["Open /health", "Log in with the seeded admin account"]

[source]
kind = "git"
url = "https://example.com/repo.git"
ref = "refs/tags/v1.0.0"

[deploy]
method = "tbuild"
default_port = 8080
health_check_path = "/health"

[[deploy.volumes]]
container_path = "/var/lib/demo"
description = "Persistent application data"
"#,
        )
        .unwrap_or_else(|err| panic!("expected rich manifest to parse: {err}"));

        assert_eq!(
            manifest.maintainers,
            vec!["tond".to_string(), "community".to_string()],
            "manifest should preserve listed maintainers"
        );
        assert_eq!(
            manifest.recommended_domains,
            vec!["stats.example.com".to_string()],
            "manifest should preserve recommended domains"
        );
        assert_eq!(
            manifest.post_deploy_checks.len(),
            2,
            "manifest should preserve post-deploy checks"
        );
        assert_eq!(
            manifest.deploy.volumes,
            vec![TemplateVolume {
                container_path: "/var/lib/demo".to_string(),
                description: "Persistent application data".to_string(),
            }],
            "manifest should preserve declared volumes"
        );
    }

    #[test]
    fn validate_rejects_relative_volume_paths() {
        let err = TemplateManifest::from_toml_str(
            Path::new("invalid-volume.toml"),
            r#"
id = "demo"
name = "Demo"
description = "Demo template"
category = "analytics"

[source]
kind = "image"
url = "ghcr.io/example/demo"

[deploy]
method = "image"
default_port = 8080

[[deploy.volumes]]
container_path = "data"
description = "Broken path"
"#,
        )
        .expect_err("relative template volume paths should be rejected");

        match err {
            TemplateManifestError::Invalid { message, .. } => assert_eq!(
                message, "template volumes must use absolute container paths",
                "validation should explain invalid volume paths clearly"
            ),
            other => panic!("expected invalid manifest error, got {other}"),
        }
    }

    #[test]
    fn manifest_supports_project_and_named_dependencies() {
        let manifest = TemplateManifest::from_toml_str(
            Path::new("project-template.toml"),
            r#"
id = "analytics"
name = "Analytics"
description = "Project-scoped analytics stack"
category = "analytics"

[project]
name = "analytics"
description = "Shared analytics services"

[source]
kind = "image"
url = "ghcr.io/example/analytics"

[deploy]
method = "image"
default_port = 3000

[[dependencies]]
name = "analytics-db"
kind = "database"
engine = "postgres"
required = true
"#,
        )
        .unwrap_or_else(|err| panic!("expected project template to parse: {err}"));

        assert_eq!(
            manifest.project,
            Some(TemplateProject {
                name: "analytics".to_string(),
                description: Some("Shared analytics services".to_string()),
            }),
            "project templates should preserve the project metadata block"
        );
        assert_eq!(
            manifest.dependencies[0].name.as_deref(),
            Some("analytics-db"),
            "named dependencies should preserve their template-local identifier"
        );
    }
}
