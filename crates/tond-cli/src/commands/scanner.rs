use std::path::Path;

use anyhow::Result;
use clap::{Args, Subcommand};

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

pub async fn handle(action: ScannerAction) -> Result<()> {
    match action {
        ScannerAction::Explain(options) => explain(options),
    }
}

fn explain(options: ExplainOptions) -> Result<()> {
    let repo_root = Path::new(&options.path);
    let build_root = options
        .base_dir
        .as_deref()
        .map(|base_dir| repo_root.join(base_dir))
        .unwrap_or_else(|| repo_root.to_path_buf());
    let output = tond_scanner::explain_in_dir(repo_root, &build_root, options.static_build)?;

    println!("Repo root: {}", output.repo_root);
    println!("Build root: {}", output.build_root);
    println!("Static build: {}", output.static_build);

    if output.matches.is_empty() {
        println!("Detected stack: none");
    } else {
        println!("Candidates:");
        for candidate in &output.matches {
            println!(
                "- {} (stack={}, confidence={})",
                candidate.profile_name, candidate.stack, candidate.confidence
            );
            for reason in &candidate.reasons {
                println!("  reason: {}", reason);
            }
        }
    }

    if let Some(plan) = output.build_plan {
        println!("Selected stack: {}", plan.stack);
        println!("Confidence: {}", plan.confidence);
        if let Some(builder_image) = plan.builder_image {
            println!("Builder image: {}", builder_image);
        }
        if let Some(runtime_image) = plan.runtime_image {
            println!("Runtime image: {}", runtime_image);
        }
        if let Some(run_command) = plan.run_command {
            println!("Run command: {}", run_command);
        }
        if let Some(static_output_dir) = plan.static_output_dir {
            println!("Static output dir: {}", static_output_dir);
        }
        println!("Detection reasons:");
        for reason in &plan.detection_reasons {
            println!("- {}", reason);
        }
    }

    if let Some(failure_reason) = output.failure_reason {
        println!("Failure: {}", failure_reason);
    }

    Ok(())
}
