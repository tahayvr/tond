use std::path::Path;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum ScannerAction {
    /// Explain how TBuild detects and plans a repository build
    Explain(ExplainOptions),
}

#[derive(Args)]
pub struct ExplainOptions {
    /// Path to the repository root
    pub path: String,

    /// Treat the repository as a static build
    #[arg(long)]
    pub static_build: bool,

    /// Optional build root inside the repository
    #[arg(long)]
    pub base_dir: Option<String>,
}

pub async fn handle(out: &Output, action: ScannerAction) -> Result<()> {
    match action {
        ScannerAction::Explain(options) => explain(out, options),
    }
}

fn explain(out: &Output, options: ExplainOptions) -> Result<()> {
    let repo_root = Path::new(&options.path);
    let build_root = options
        .base_dir
        .as_deref()
        .map(|base_dir| repo_root.join(base_dir))
        .unwrap_or_else(|| repo_root.to_path_buf());
    let output = tond_scanner::explain_in_dir(repo_root, &build_root, options.static_build)?;

    out.json(&json!({
        "repo_root": output.repo_root,
        "build_root": output.build_root,
        "static_build": output.static_build,
        "matches": output.matches.iter().map(|m| json!({
            "profile_name": m.profile_name,
            "stack": m.stack,
            "confidence": m.confidence,
            "reasons": m.reasons,
        })).collect::<Vec<_>>(),
        "build_plan": output.build_plan.as_ref().map(|plan| json!({
            "profile_name": plan.profile_name,
            "stack": plan.stack,
            "confidence": plan.confidence,
            "detection_reasons": plan.detection_reasons,
            "builder_image": plan.builder_image,
            "runtime_image": plan.runtime_image,
            "run_command": plan.run_command,
            "static_output_dir": plan.static_output_dir,
            "dockerfile": plan.dockerfile,
        })),
        "failure_reason": output.failure_reason,
    }))?;

    outln!(out, "Repo root: {}", output.repo_root);
    outln!(out, "Build root: {}", output.build_root);
    outln!(out, "Static build: {}", output.static_build);

    if output.matches.is_empty() {
        outln!(out, "Detected stack: none");
    } else {
        outln!(out, "Candidates:");
        for candidate in &output.matches {
            outln!(
                out,
                "- {} (stack={}, confidence={})",
                candidate.profile_name,
                candidate.stack,
                candidate.confidence
            );
            for reason in &candidate.reasons {
                outln!(out, "  reason: {}", reason);
            }
        }
    }

    if let Some(plan) = output.build_plan {
        outln!(out, "Selected stack: {}", plan.stack);
        outln!(out, "Confidence: {}", plan.confidence);
        if let Some(builder_image) = plan.builder_image {
            outln!(out, "Builder image: {}", builder_image);
        }
        if let Some(runtime_image) = plan.runtime_image {
            outln!(out, "Runtime image: {}", runtime_image);
        }
        if let Some(run_command) = plan.run_command {
            outln!(out, "Run command: {}", run_command);
        }
        if let Some(static_output_dir) = plan.static_output_dir {
            outln!(out, "Static output dir: {}", static_output_dir);
        }
        outln!(out, "Detection reasons:");
        for reason in &plan.detection_reasons {
            outln!(out, "- {}", reason);
        }
    }

    if let Some(failure_reason) = output.failure_reason {
        outln!(out, "Failure: {}", failure_reason);
    }

    Ok(())
}
