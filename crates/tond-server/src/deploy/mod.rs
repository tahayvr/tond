// deploy/mod.rs — deployment orchestration module
//
//   core.rs     — start()/create()/run() (the deployment pipeline),
//                 deploy_preview() and internal helpers
//   build.rs    — image build helpers (git clone, tbuild, dockerfile, compose)
//   git.rs      — clone hardening: credentials via env, log redaction,
//                 known_hosts; commit and branch listing helpers
//   progress.rs — phase tracking and log attribution for the active deployment
//   queue.rs    — the build/deploy queue: one deploy per app, coalescing, a
//                 global build semaphore; deploy::start() queues through it
//   failure.rs  — classify a failed deployment into a likely cause + fix hint
//   images.rs   — per-deployment image tags and pruning
//   template_setup.rs — a template's post_deploy steps (pre-seed the admin)
//                 before the first release routes the app
//
// Public surface:
//   deploy::start(state, app, DeployRequest) -> queue::Enqueued (id, place in line)
//   deploy::create(..) + deploy::run(.., ctx) — the halves (run by the queue)
//   deploy::deploy_preview(state, app, preview)

mod build;
mod compose;
mod core;
pub mod failure;
pub mod git;
pub mod images;
pub mod progress;
pub mod queue;
pub mod template_setup;

pub use build::{compose_project_name, compose_work_dir_for, validate_base_dir};
pub use compose::{compose_path_in_repo, deployment_services};
pub use core::{
    DeployKind, DeployRequest, deploy_preview, fetch_registry_credentials, pull_image, run, start,
};
pub use git::{GitHttpAuth, redact_url_credentials};
