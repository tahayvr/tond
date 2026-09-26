// auth.rs — authentication and authorization
//
// Sub-modules:
//   principal       — who is calling (user, API token, local socket) and the
//                     `Authz<scope::…>` extractor every handler uses
//   tokens          — scoped API tokens (`tond_pat_…`)
//   password_policy — the one password policy (length + breached list)
//   extractor       — `LocalSocket` / `LocalPeer` (the root-only Unix socket)
//   csrf, rate_limit, setup_token, secret_file — browser protections,
//                     throttling and the first-run setup token
//
// The role → scope mapping lives in tond_common::access (one place).
// There is no root API key: bootstrap automation with
// `tond token create --name ci --preset deployer --app <app>` on the host.

pub mod csrf;
pub mod extractor;
pub mod password_policy;
pub mod principal;
pub mod rate_limit;
pub mod secret_file;
pub mod setup_token;
pub mod tokens;

#[cfg(test)]
mod authz_tests;

pub use extractor::{LocalPeer, LocalSocket};
pub use principal::{Authz, MaybePrincipal, Principal, scope};
pub use rate_limit::{ClientIp, RateLimit, RateLimiter};
