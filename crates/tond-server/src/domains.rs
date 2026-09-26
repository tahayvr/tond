// domains.rs — domain validation for the dashboard and preview domains
//
// Previews run pull-request code, which may come from people who cannot
// otherwise touch the server. They are therefore served only on a separate
// *preview domain* whose registrable domain ("site") differs from the
// dashboard's. Same-site pages can set cookies for the parent domain, send
// SameSite=Lax/Strict cookies along with requests to the dashboard API, and
// are treated as same-site by browsers' CSRF defences — a cross-site preview
// gets none of that.
//
// "Registrable domain" is computed with the Public Suffix List (`psl` crate,
// list compiled into the binary): for `dash.example.co.uk` it is
// `example.co.uk`, for `foo.github.io` it is `foo.github.io`. A naive
// "last two labels" check would treat every `*.co.uk` domain as one site and
// every `*.github.io` user page as the same site as `github.io`. Names under
// TLDs that are not on the list (e.g. `.test`, used by the e2e tests) fall
// back to the list's default `*` rule: the last label is the suffix.
//
//   normalize_domain()        trim, lowercase, drop a trailing dot
//   registrable_domain()      the PSL registrable domain of a hostname
//   validate_preview_domain() check a preview domain against the dashboard domain
//   preview_host()            <slug>-<app>.<preview_domain>
//   require_preview_domain()  the configured preview domain, or a 400 explaining why one is needed

use tond_common::TondError;

use crate::{error::AppError, state::AppState};

/// Returned whenever previews are requested without a preview domain.
pub const PREVIEW_DOMAIN_REQUIRED: &str = "Previews need a preview domain. Set one in Settings: \
     a separate domain you own (e.g. example-previews.com, not a subdomain of the dashboard \
     domain) with a wildcard DNS record *.<preview-domain> pointing at this server. Previews run \
     pull-request code, so they must not share a site with the dashboard.";

/// Trim whitespace, lowercase and drop a single trailing dot.
pub fn normalize_domain(input: &str) -> String {
    let d = input.trim().to_ascii_lowercase();
    d.strip_suffix('.').map(str::to_string).unwrap_or(d)
}

/// A syntactically valid DNS hostname with at least two labels and a
/// non-numeric last label (so IPv4 addresses are rejected).
pub fn is_valid_hostname(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !labels.iter().all(|l| label_ok(l)) {
        return false;
    }
    labels
        .last()
        .is_some_and(|tld| !tld.bytes().all(|b| b.is_ascii_digit()))
}

/// The registrable domain ("site") of a hostname per the Public Suffix List,
/// or `None` when the hostname is itself a public suffix (e.g. `co.uk`).
pub fn registrable_domain(domain: &str) -> Option<String> {
    psl::domain_str(domain).map(str::to_string)
}

/// `a` equals `b` or is a subdomain of it.
fn is_same_or_subdomain(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}"))
}

/// Validate a (normalized) preview domain against the (normalized) dashboard
/// domain, which may be empty when the dashboard is only reached by IP.
pub fn validate_preview_domain(preview: &str, dashboard: &str) -> Result<(), String> {
    if !is_valid_hostname(preview) {
        return Err(format!(
            "'{preview}' is not a valid domain name (expected something like example-previews.com)"
        ));
    }
    let Some(preview_site) = registrable_domain(preview) else {
        return Err(format!(
            "'{preview}' is a public suffix; use a domain you own, such as example-previews.com"
        ));
    };
    if dashboard.is_empty() {
        return Ok(());
    }
    if is_same_or_subdomain(preview, dashboard) || is_same_or_subdomain(dashboard, preview) {
        return Err(format!(
            "The preview domain must not be the dashboard domain ({dashboard}) or a parent or \
             subdomain of it. Previews run pull-request code; use a separate domain you own."
        ));
    }
    let dashboard_site = registrable_domain(dashboard).unwrap_or_else(|| dashboard.to_string());
    if preview_site == dashboard_site {
        return Err(format!(
            "The preview domain must be a different registrable domain from the dashboard: both \
             {preview} and {dashboard} belong to {preview_site}, so previews would be same-site \
             with the dashboard (shared cookies, same-site requests to the API). Use a separate \
             domain you own, such as {}-previews.{}.",
            preview_site.split('.').next().unwrap_or("example"),
            preview_site
                .split_once('.')
                .map(|(_, s)| s)
                .unwrap_or("com"),
        ));
    }
    Ok(())
}

/// The hostname a preview is served at: `<slug>-<app>.<preview_domain>`.
pub fn preview_host(slug: &str, app_name: &str, preview_domain: &str) -> String {
    format!("{slug}-{app_name}.{preview_domain}")
}

/// The configured preview domain, or a 400 that explains why previews need one.
pub async fn require_preview_domain(state: &AppState) -> Result<String, AppError> {
    let preview_domain = state.settings.read().await.preview_domain.clone();
    if preview_domain.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            PREVIEW_DOMAIN_REQUIRED.to_string(),
        )));
    }
    Ok(preview_domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_domain_trims_lowercases_and_drops_trailing_dot() {
        assert_eq!(
            normalize_domain("  Preview.Example.COM. "),
            "preview.example.com"
        );
        assert_eq!(normalize_domain(""), "");
    }

    #[test]
    fn is_valid_hostname_rejects_malformed_names_and_ips() {
        assert!(is_valid_hostname("example-previews.com"));
        assert!(is_valid_hostname("preview.test"));
        assert!(!is_valid_hostname("localhost"), "single label");
        assert!(!is_valid_hostname("203.0.113.7"), "IPv4 address");
        assert!(!is_valid_hostname("*.example.com"), "wildcard");
        assert!(!is_valid_hostname("-bad.example.com"));
        assert!(!is_valid_hostname("a..b.com"));
        assert!(!is_valid_hostname("https://example.com"));
    }

    #[test]
    fn registrable_domain_uses_the_public_suffix_list() {
        assert_eq!(
            registrable_domain("dash.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(
            registrable_domain("a.b.github.io").as_deref(),
            Some("b.github.io"),
            "private-section suffixes like github.io make each user page its own site"
        );
        assert_eq!(registrable_domain("co.uk"), None);
        assert_eq!(
            registrable_domain("dash.tond.test").as_deref(),
            Some("tond.test"),
            "unlisted TLDs fall back to the default '*' rule"
        );
    }

    #[test]
    fn validate_preview_domain_accepts_a_separate_site() {
        assert_eq!(
            validate_preview_domain("example-previews.com", "dash.example.com"),
            Ok(())
        );
        assert_eq!(
            validate_preview_domain("preview.test", "dash.tond.test"),
            Ok(())
        );
        assert_eq!(
            validate_preview_domain("pr.example-previews.com", "dash.example.com"),
            Ok(()),
            "a subdomain of another site is fine"
        );
        assert_eq!(
            validate_preview_domain("example-previews.com", ""),
            Ok(()),
            "no dashboard domain (dashboard reached by IP) is not same-site with anything"
        );
    }

    #[test]
    fn validate_preview_domain_rejects_same_site_and_parent_child_domains() {
        for (preview, dashboard) in [
            ("dash.example.com", "dash.example.com"),
            ("previews.dash.example.com", "dash.example.com"),
            ("example.com", "dash.example.com"),
            ("previews.example.com", "dash.example.com"),
            ("previews.example.co.uk", "dash.example.co.uk"),
        ] {
            assert!(
                validate_preview_domain(preview, dashboard).is_err(),
                "{preview} must be rejected next to dashboard {dashboard}"
            );
        }
    }

    #[test]
    fn validate_preview_domain_rejects_public_suffixes_and_invalid_names() {
        assert!(validate_preview_domain("co.uk", "dash.example.com").is_err());
        assert!(validate_preview_domain("github.io", "dash.example.com").is_err());
        assert!(validate_preview_domain("not a domain", "").is_err());
    }

    #[test]
    fn preview_host_prefixes_slug_and_app_name() {
        assert_eq!(
            preview_host("abc123", "frontend", "example-previews.com"),
            "abc123-frontend.example-previews.com"
        );
    }
}
