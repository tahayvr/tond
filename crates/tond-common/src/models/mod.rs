// models/mod.rs — module declarations and flat re-exports
//
// All domain types are split into focused sub-modules. Everything is
// re-exported here so that consumers can still write `use tond_common::App`
// (or `use tond_common::*`) without knowing which sub-module a type lives in.

pub mod access;
pub mod alerts;
pub mod api;
pub mod apps;
pub mod audit_shipping;
pub mod auth;
pub mod backup;
pub mod certificates;
pub mod cron;
pub mod databases;
pub mod db_catalog;
pub mod deployments;
pub mod domains;
pub mod events;
pub mod gitea;
pub mod github;
pub mod gitlab;
pub mod monitoring;
pub mod notifications;
pub mod projects;
pub mod registry;
pub mod requests;
pub mod runtime;
pub mod settings;
pub mod storage;
pub mod templates;

#[cfg(test)]
mod secret_fields_test;

pub use access::*;
pub use alerts::*;
pub use api::*;
pub use apps::*;
pub use audit_shipping::*;
pub use auth::*;
pub use backup::*;
pub use certificates::*;
pub use cron::*;
pub use databases::*;
pub use db_catalog::*;
pub use deployments::*;
pub use domains::*;
pub use events::*;
pub use gitea::*;
pub use github::*;
pub use gitlab::*;
pub use monitoring::*;
pub use notifications::*;
pub use projects::*;
pub use registry::*;
pub use requests::*;
pub use runtime::*;
pub use settings::*;
pub use storage::*;
pub use templates::*;
