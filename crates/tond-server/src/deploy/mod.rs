// deploy/mod.rs — deployment orchestration module
//
// Splits the original deploy.rs into focused sub-modules:
//   core.rs  — deploy(), deploy_image(), deploy_preview() and internal helpers
//   build.rs — all image build helpers (git clone, tbuild, dockerfile, compose)
//   git.rs   — clone hardening: credentials via env, log redaction, known_hosts
//   queue.rs — the build/deploy queue: one deploy per app, coalescing, a
//              global build semaphore; every production deploy goes through it
//
// Public surface:
//   deploy::queue::enqueue(state, app, kind, trigger_source) — start a deploy
//   deploy::deploy_preview(state, app, preview)
//   deploy::deploy / deploy_image / build_from_dir are run by the queue

mod build;
mod core;
mod git;
pub mod images;
pub mod queue;

pub use build::{build_from_dir, compose_project_name, validate_base_dir};
pub use core::{deploy, deploy_image, deploy_preview};
pub use git::GitHttpAuth;
