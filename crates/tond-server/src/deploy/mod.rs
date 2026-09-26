// deploy/mod.rs — deployment orchestration module
//
// Splits the original deploy.rs into focused sub-modules:
//   core.rs  — deploy(), deploy_image(), deploy_preview() and internal helpers
//   build.rs — all image build helpers (git clone, tbuild, dockerfile, compose)
//   git.rs   — clone hardening: credentials via env, log redaction, known_hosts
//
// Public surface (unchanged from the original deploy.rs):
//   deploy::deploy(state, app, trigger_source)
//   deploy::deploy_image(state, app, image_tag)
//   deploy::deploy_preview(state, app, preview)
//   deploy::build_from_dir(state, app, dir)

mod build;
mod core;
mod git;

pub use build::{build_from_dir, validate_base_dir};
pub use core::{deploy, deploy_image, deploy_preview};
pub use git::GitHttpAuth;
