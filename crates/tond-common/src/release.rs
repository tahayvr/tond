// release.rs — where Tond releases are downloaded from, and what they are called
//
// One release is a GitHub release of tahayvr/tond tagged `v<semver>` with, per
// architecture (x86_64, aarch64):
//
//   tond-linux-<arch>.tar.gz              the bundle (tond-linux-<arch>/…, VERSION, ARCH)
//   tond-linux-<arch>.tar.gz.sha256       "<sha256>  tond-linux-<arch>.tar.gz"
//   tond-linux-<arch>.tar.gz.minisig      minisign signature of the tarball
//   tond-linux-<arch>.tar.gz.sha256.minisig
//
// plus minisign.pub. The release base is `TOND_RELEASE_BASE` (env), else the
// first line of `<install dir>/RELEASE_BASE` (written by install.sh when it was
// run with a non-default base), else DEFAULT_RELEASE_BASE. Assets are at
// `<base>/download/<tag>/<file>`; `<base>/latest` names the latest release
// (GitHub redirects it to `…/releases/tag/<tag>`; a plain file server serves a
// file whose first line is the tag). The shell scripts (release/install.sh,
// update.sh, web/install.sh, web/update.sh) share the same rules in their
// "release signatures" block; the tests below keep the copies in step with
// this file and with .github/workflows/release.yml.

use std::path::Path;

/// The public GitHub releases of this repository.
pub const DEFAULT_RELEASE_BASE: &str = "https://github.com/tahayvr/tond/releases";
/// Environment variable that overrides the release base.
pub const RELEASE_BASE_ENV: &str = "TOND_RELEASE_BASE";
/// File (in the install directory) that records a non-default release base.
pub const RELEASE_BASE_FILE: &str = "RELEASE_BASE";
/// The architectures releases are built for (bundle names `tond-linux-<arch>`).
pub const RELEASE_ARCHES: [&str; 2] = ["x86_64", "aarch64"];

/// The release base from an environment value and the RELEASE_BASE file's
/// content (either may be absent or blank), without a trailing slash.
pub fn release_base_from(env: Option<&str>, file: Option<&str>) -> String {
    let pick = |v: Option<&str>| {
        v.and_then(|s| s.lines().next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    pick(env)
        .or_else(|| pick(file))
        .unwrap_or_else(|| DEFAULT_RELEASE_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The release base for an install at `install_dir` (usually /opt/tond).
pub fn release_base(install_dir: &Path) -> String {
    let env = std::env::var(RELEASE_BASE_ENV).ok();
    let file = std::fs::read_to_string(install_dir.join(RELEASE_BASE_FILE)).ok();
    release_base_from(env.as_deref(), file.as_deref())
}

/// Bundle name for an architecture, e.g. `tond-linux-aarch64`.
pub fn bundle_name(arch: &str) -> String {
    format!("tond-linux-{arch}")
}

/// URL of a release asset: `<base>/download/<tag>/<file>`.
pub fn asset_url(base: &str, tag: &str, file: &str) -> String {
    format!("{}/download/{tag}/{file}", base.trim_end_matches('/'))
}

/// URL that names the latest release: `<base>/latest`.
pub fn latest_url(base: &str) -> String {
    format!("{}/latest", base.trim_end_matches('/'))
}

/// A release tag the scripts accept: `v` + a digit, then only
/// `[A-Za-z0-9.+-]` (it ends up in a URL path).
pub fn is_valid_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    chars.next() == Some('v')
        && chars.next().is_some_and(|c| c.is_ascii_digit())
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// The tag `<base>/latest` points at: from the final URL after redirects
/// (`…/releases/tag/<tag>`), else the first line of the body.
pub fn tag_from_latest(final_url: &str, body: &str) -> Option<String> {
    let from_url = final_url
        .trim()
        .rsplit_once("/releases/tag/")
        .and_then(|(_, t)| t.split(['?', '#', '/']).next())
        .map(str::to_string);
    let from_body = || body.lines().next().map(|l| l.trim().to_string());
    from_url
        .filter(|t| is_valid_tag(t))
        .or_else(|| from_body().filter(|t| is_valid_tag(t)))
}

/// The releases list (GitHub's releases API format) for a release base:
/// `https://github.com/<owner>/<repo>/releases` → the GitHub API, anything
/// else → `<base>/releases.json` (a mirror serves the same JSON there).
pub fn releases_api_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://github.com/")
        && let Some(repo) = rest.strip_suffix("/releases")
        && repo.split('/').count() == 2
    {
        return format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    }
    format!("{base}/releases.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_prefers_env_then_file_then_default() {
        assert_eq!(release_base_from(None, None), DEFAULT_RELEASE_BASE);
        assert_eq!(
            release_base_from(Some(" "), Some("http://10.0.0.2:8000/r/\n")),
            "http://10.0.0.2:8000/r"
        );
        assert_eq!(
            release_base_from(Some("https://m.example/tond/"), Some("http://x")),
            "https://m.example/tond"
        );
    }

    #[test]
    fn urls() {
        assert_eq!(
            asset_url(DEFAULT_RELEASE_BASE, "v0.2.0", "tond-linux-x86_64.tar.gz"),
            "https://github.com/tahayvr/tond/releases/download/v0.2.0/tond-linux-x86_64.tar.gz"
        );
        assert_eq!(
            latest_url("http://127.0.0.1:8000/"),
            "http://127.0.0.1:8000/latest"
        );
        assert_eq!(
            releases_api_url(DEFAULT_RELEASE_BASE),
            "https://api.github.com/repos/tahayvr/tond/releases?per_page=30"
        );
        assert_eq!(
            releases_api_url("https://m.example/tond"),
            "https://m.example/tond/releases.json"
        );
    }

    #[test]
    fn tags() {
        assert!(is_valid_tag("v0.2.0"));
        assert!(is_valid_tag("v1.0.0-rc.1"));
        assert!(!is_valid_tag("0.2.0"));
        assert!(!is_valid_tag("v"));
        assert!(!is_valid_tag("v1/../x"));
        assert_eq!(
            tag_from_latest(
                "https://github.com/tahayvr/tond/releases/tag/v0.2.0\n",
                "<html>"
            )
            .as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(
            tag_from_latest("http://127.0.0.1:8000/latest", "v0.3.1\n").as_deref(),
            Some("v0.3.1")
        );
        // No releases yet: GitHub redirects to the release list.
        assert_eq!(
            tag_from_latest(
                "https://github.com/tahayvr/tond/releases",
                "<!DOCTYPE html>"
            ),
            None
        );
    }

    // ── The scripts and the release workflow agree with this file ──

    const SCRIPTS: [(&str, &str); 4] = [
        (
            "release/install.sh",
            include_str!("../../../release/install.sh"),
        ),
        (
            "release/update.sh",
            include_str!("../../../release/update.sh"),
        ),
        (
            "release/web/install.sh",
            include_str!("../../../release/web/install.sh"),
        ),
        (
            "release/web/update.sh",
            include_str!("../../../release/web/update.sh"),
        ),
    ];
    const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");
    const PACKAGE_SH: &str = include_str!("../../../release/package.sh");

    fn signature_block(script: &str) -> &str {
        let start = script
            .find("# ── Release signatures (keep in sync")
            .expect("release signatures block");
        let end = script[start..]
            .find("# ── end release signatures ──")
            .expect("end of the release signatures block");
        &script[start..start + end]
    }

    /// The four scripts share one release block (download base, URL layout,
    /// latest-tag lookup, checksum and signature policy) byte for byte.
    #[test]
    fn scripts_share_one_release_block() {
        let first = signature_block(SCRIPTS[0].1);
        for (name, script) in &SCRIPTS[1..] {
            assert!(
                signature_block(script) == first,
                "{name}: the release signatures block differs from release/install.sh's; keep the copies identical"
            );
        }
        assert!(
            first.contains(&format!(
                "TOND_DEFAULT_RELEASE_BASE=\"{DEFAULT_RELEASE_BASE}\""
            )),
            "the scripts' default release base must be {DEFAULT_RELEASE_BASE}"
        );
        assert!(first.contains("\"${TOND_INSTALL_DIR:-/opt/tond}/RELEASE_BASE\""));
        // Asset layout: <base>/download/<tag>/<file>, latest at <base>/latest.
        assert!(first.contains("'%s/download/%s/%s\\n'"));
        assert!(first.contains("/latest\""));
        // What the scripts fetch for a bundle.
        for suffix in [".tar.gz\"", ".sha256\"", ".minisig\""] {
            assert!(first.contains(suffix), "the block must fetch {suffix}");
        }
    }

    /// No script builds a download URL outside the shared block.
    #[test]
    fn scripts_build_urls_only_in_the_block() {
        for (name, script) in SCRIPTS {
            let block = signature_block(script);
            let rest = script.replacen(block, "", 1);
            for needle in [
                "releases/download",
                "api.github.com/repos/tahayvr",
                "/releases/latest",
            ] {
                assert!(
                    !rest.contains(needle),
                    "{name} builds a release URL ({needle}) outside the release signatures block"
                );
            }
            assert!(
                rest.contains("fetch_verified_release "),
                "{name} must download through fetch_verified_release"
            );
        }
    }

    /// The workflow builds, signs and publishes exactly the assets the
    /// scripts and `tond update --direct` download, for both architectures.
    #[test]
    fn workflow_publishes_what_clients_download() {
        for arch in RELEASE_ARCHES {
            assert!(
                WORKFLOW.contains(&format!("arch: {arch}")),
                "release.yml must build {arch}"
            );
            let bundle = bundle_name(arch);
            for file in [
                format!("{bundle}.tar.gz"),
                format!("{bundle}.tar.gz.sha256"),
                format!("{bundle}.tar.gz.minisig"),
                format!("{bundle}.tar.gz.sha256.minisig"),
            ] {
                assert!(
                    WORKFLOW.contains(&format!("dist/{file}\n")),
                    "release.yml must publish {file}"
                );
            }
        }
        assert!(WORKFLOW.contains("release/minisign.pub"));
        // The bundle's layout: what package.sh writes is what the scripts unpack.
        assert!(PACKAGE_SH.contains("NAME=\"tond-linux-${ARCH}\""));
        assert!(
            PACKAGE_SH.contains("tar -czf \"${BUNDLE}.tar.gz\" -C \"${ROOT}/dist\" \"${NAME}\"")
        );
        assert!(PACKAGE_SH.contains("echo \"${VERSION}\" > \"${BUNDLE}/VERSION\""));
        // The tag decides VERSION (without the v) and must match Cargo.toml.
        assert!(WORKFLOW.contains("scripts/check-version.sh"));
    }
}
