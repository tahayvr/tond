// profiles.rs — LanguageProfile registry
//
// Profiles are probed in declaration order. The first profile whose `detect()`
// returns `true` is used. Add new profiles here once they implement
// `LanguageProfile` in their own file under `src/profiles/`.

mod go;
mod node;
mod python;
mod rust;

use std::path::Path;

use crate::profile::{DetectionOutcome, LanguageProfile};

pub(crate) struct ProfileMatch {
    pub profile: Box<dyn LanguageProfile>,
    pub stack: String,
    pub confidence: u8,
    pub reasons: Vec<String>,
}

/// Return all matching [`LanguageProfile`]s ordered by confidence, highest first.
pub(crate) fn detect(repo_path: &Path) -> Vec<ProfileMatch> {
    let profiles: Vec<Box<dyn LanguageProfile>> = vec![
        Box::new(node::NodeJsProfile),
        Box::new(python::PythonProfile),
        Box::new(go::GoProfile),
        Box::new(rust::RustProfile),
    ];

    let mut matches: Vec<ProfileMatch> = profiles
        .into_iter()
        .filter_map(|profile| {
            let DetectionOutcome {
                confidence,
                reasons,
            } = profile.detect_with_confidence(repo_path)?;
            Some(ProfileMatch {
                stack: profile.name().to_ascii_lowercase().replace(['.', ' '], "-"),
                profile,
                confidence,
                reasons,
            })
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| left.profile.name().cmp(right.profile.name()))
    });
    matches
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn create_repo() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temporary repo: {err}"))
    }

    #[test]
    fn detect_returns_none_when_no_profile_matches() {
        let repo = create_repo();

        let detected = detect(repo.path());

        assert!(
            detected.is_empty(),
            "detect should return None when no supported stack markers are present"
        );
    }

    #[test]
    fn detect_prefers_node_profile_when_multiple_markers_exist() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(
            repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));

        let detected = detect(repo.path());

        assert_eq!(
            detected
                .first()
                .unwrap_or_else(|| panic!("expected a profile to be detected for mixed markers"))
                .profile
                .name(),
            "Node.js",
            "profiles should be evaluated in registry order so Node.js wins over later profiles"
        );
    }

    #[test]
    fn detect_returns_rust_profile_for_cargo_projects() {
        let repo = create_repo();
        fs::write(
            repo.path().join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write Cargo.toml: {err}"));

        let detected = detect(repo.path());

        assert_eq!(
            detected
                .first()
                .unwrap_or_else(|| panic!("expected a profile to be detected for a Cargo project"))
                .profile
                .name(),
            "Rust",
            "Cargo projects should be detected by the Rust profile"
        );
    }
}
