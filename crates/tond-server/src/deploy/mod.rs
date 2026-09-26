// deploy/mod.rs — deployment orchestration module
//
//   core.rs     — start()/create()/run() (the deployment pipeline),
//                 deploy_preview() and internal helpers
//   build.rs    — image build helpers (git clone, tbuild, dockerfile, compose)
//   git.rs      — clone hardening: credentials via env, log redaction,
//                 known_hosts; commit and branch listing helpers
//   progress.rs — phase tracking and log attribution for the active deployment
//   failure.rs  — classify a failed deployment into a likely cause + fix hint
//   images.rs   — per-deployment image tags and pruning
//
// Public surface:
//   deploy::start(state, app, DeployRequest) -> deployment id (runs in background)
//   deploy::create(..) + deploy::run(..)   — the same, run in the caller's task
//   deploy::deploy_preview(state, app, preview)

mod build;
mod core;
pub mod failure;
pub mod git;
pub mod images;
pub mod progress;

pub use build::{compose_project_name, validate_base_dir};
pub use core::{
    DeployKind, DeployRequest, create, deploy_preview, fetch_registry_credentials, pull_image, run,
    start,
};
pub use git::GitHttpAuth;
