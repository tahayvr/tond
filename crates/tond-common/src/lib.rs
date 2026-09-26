// tond-common — shared library crate
//
// This crate is the single source of truth for every type that crosses a
// crate boundary. Both tond-server and tond-cli depend on it so they never
// have to duplicate struct definitions or re-implement helpers.
//
// Sub-modules:
//   errors  — the unified TondError enum used across the workspace
//   dns     — compare a domain's DNS with this server's addresses
//   models  — all domain structs, DTOs, and request/response types
//   traffic — per-route request counters (tond-proxy -> tond-server)
//
// `dns` is used as `tond_common::dns::…` (not re-exported flat).
//
// errors and models are re-exported flat from the crate root so consumers can write
// `use tond_common::App` instead of `use tond_common::models::App`.

pub mod dns;
pub mod errors;
pub mod models;
pub mod procstat;
pub mod proxy_control;
pub mod traffic;

pub use errors::*;
pub use models::*;

/// Default path of tond-server's local Unix socket (the sibling of the default
/// database path). The `tond` CLI connects here; requests on this socket are
/// trusted without credentials, so it is created with mode 0600 (root only).
pub const DEFAULT_SOCKET_PATH: &str = "/opt/tond/data/tond.sock";

/// Install aws-lc-rs as the process-wide rustls crypto provider. Call first thing in `main`.
///
/// Dependencies disagree on rustls backends (lettre enables `ring`; instant-acme and
/// reqwest enable `aws-lc-rs`). When the binaries are built in one cargo invocation,
/// feature unification compiles both in, and rustls then refuses to pick a default and
/// panics on first use — which crashed tond-proxy whenever it started ACME provisioning.
/// Installing a provider explicitly makes every binary independent of how it was built.
pub fn install_crypto_provider() {
    // Err only means a provider is already installed, which is fine.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
