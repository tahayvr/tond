// domains.rs — domain validation for the dashboard, apps and preview domains
//
// Three kinds of hostnames, three sites:
//
//   dashboard domain  the dashboard and API (session cookies live here)
//   apps domain       automatic app hostnames, `<app>.<apps_domain>`
//   preview domain    pull-request previews, `<slug>-<app>.<preview_domain>`
//
// Apps run user code and previews run pull-request code, so each must be a
// different registrable domain from the dashboard — and from each other: a
// preview on the apps' site could set cookies for production apps, and
// tond-proxy reserves the preview domain for previews (apps under it are not
// routed). Custom app domains may share the dashboard's site (the user owns
// both), but the dashboard shows a warning.
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
//   validate_apps_domain()    check an apps domain against the dashboard and preview domains
//   check_custom_app_domain() validate a custom app domain; Ok(Some(warning)) when same-site
//                             with the dashboard
//   app_host()                <app>.<apps_domain>
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

/// `a` and `b` are the same site: one is the other or a subdomain of it, or
/// both have the same registrable domain.
pub fn same_site(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if is_same_or_subdomain(a, b) || is_same_or_subdomain(b, a) {
        return true;
    }
    let site_a = registrable_domain(a).unwrap_or_else(|| a.to_string());
    let site_b = registrable_domain(b).unwrap_or_else(|| b.to_string());
    site_a == site_b
}

/// Validate a (normalized) apps domain against the (normalized) dashboard and
/// preview domains; either may be empty.
pub fn validate_apps_domain(apps: &str, dashboard: &str, preview: &str) -> Result<(), String> {
    if !is_valid_hostname(apps) {
        return Err(format!(
            "'{apps}' is not a valid domain name (expected something like example-apps.com)"
        ));
    }
    let Some(apps_site) = registrable_domain(apps) else {
        return Err(format!(
            "'{apps}' is a public suffix; use a domain you own, such as example-apps.com"
        ));
    };
    if same_site(apps, dashboard) {
        return Err(format!(
            "The apps domain must be a different site from the dashboard ({dashboard}): {apps}              and {dashboard} both belong to {apps_site}, so every app would be same-site with              the dashboard (it could send same-site requests to the API and set cookies the              dashboard reads). Use a separate domain, such as example-apps.com."
        ));
    }
    if same_site(apps, preview) {
        return Err(format!(
            "The apps domain must be a different site from the preview domain ({preview}).              Previews run pull-request code and must not share a site with your production              apps, and hostnames under the preview domain only serve previews."
        ));
    }
    Ok(())
}

/// The automatic hostname of an app: `<app>.<apps_domain>`.
pub fn app_host(app_name: &str, apps_domain: &str) -> String {
    format!("{app_name}.{apps_domain}")
}

/// Shown next to a custom app domain on the dashboard's site.
pub fn same_site_warning(domain: &str, dashboard: &str) -> String {
    format!(
        "{domain} is on the same site as your dashboard ({dashboard}). The app can send          same-site requests to the dashboard and set cookies for it; use a domain on a          different site for code you don't fully trust."
    )
}

/// Validate a (normalized) custom app domain. Returns a warning when it is on
/// the dashboard's site (allowed: the user owns both), an error when it can't
/// work (the dashboard domain itself, or under the preview domain, which only
/// serves previews).
pub fn check_custom_app_domain(
    domain: &str,
    dashboard: &str,
    preview: &str,
) -> Result<Option<String>, String> {
    if !is_valid_hostname(domain) {
        return Err(format!(
            "'{domain}' is not a valid domain name (expected something like app.example.com)"
        ));
    }
    if !dashboard.is_empty() && domain == dashboard {
        return Err(format!(
            "{domain} is the dashboard domain; choose another hostname for the app"
        ));
    }
    if !preview.is_empty() && is_same_or_subdomain(domain, preview) {
        return Err(format!(
            "{domain} is under the preview domain ({preview}), which only serves previews;              choose a hostname outside it"
        ));
    }
    Ok(same_site(domain, dashboard).then(|| same_site_warning(domain, dashboard)))
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
    fn validate_apps_domain_requires_a_site_of_its_own() {
        assert_eq!(
            validate_apps_domain(
                "example-apps.com",
                "dash.example.com",
                "example-previews.com"
            ),
            Ok(())
        );
        assert_eq!(
            validate_apps_domain("203-0-113-7.nip.io", "203-0-113-7.sslip.io", ""),
            Ok(()),
            "the install default: apps on nip.io next to a dashboard on sslip.io"
        );
        for (apps, dashboard, preview) in [
            ("apps.example.com", "dash.example.com", ""),
            ("dash.example.com", "dash.example.com", ""),
            ("apps.203-0-113-7.sslip.io", "203-0-113-7.sslip.io", ""),
            (
                "example-previews.com",
                "dash.example.com",
                "example-previews.com",
            ),
            (
                "apps.example-previews.com",
                "dash.example.com",
                "example-previews.com",
            ),
            ("co.uk", "", ""),
            ("not a domain", "", ""),
        ] {
            assert!(
                validate_apps_domain(apps, dashboard, preview).is_err(),
                "{apps} must be rejected (dashboard {dashboard:?}, preview {preview:?})"
            );
        }
    }

    #[test]
    fn custom_app_domains_on_the_dashboard_site_are_allowed_with_a_warning() {
        assert_eq!(
            check_custom_app_domain("shop.example.org", "dash.example.com", ""),
            Ok(None)
        );
        let warning = check_custom_app_domain("shop.example.com", "dash.example.com", "")
            .unwrap_or_else(|e| panic!("same-site custom domains are allowed: {e}"));
        assert!(
            warning
                .as_deref()
                .is_some_and(|w| w.contains("same site as your dashboard")),
            "got {warning:?}"
        );
        assert!(check_custom_app_domain("dash.example.com", "dash.example.com", "").is_err());
        assert!(
            check_custom_app_domain("x.example-previews.com", "", "example-previews.com").is_err()
        );
        assert!(check_custom_app_domain("bad domain", "", "").is_err());
    }

    #[test]
    fn app_host_suffixes_the_app_name() {
        assert_eq!(app_host("web", "example-apps.com"), "web.example-apps.com");
    }

    #[test]
    fn preview_host_prefixes_slug_and_app_name() {
        assert_eq!(
            preview_host("abc123", "frontend", "example-previews.com"),
            "abc123-frontend.example-previews.com"
        );
    }
}
