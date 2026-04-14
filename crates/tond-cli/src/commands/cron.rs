// commands/cron.rs — `tond cron` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum CronAction {
    /// List all cron jobs
    List,

    /// Create a new cron job
    Create {
        /// Cron job name (alphanumeric + hyphens)
        name: String,

        /// Cron schedule (5-field, e.g. "0 * * * *" for every hour)
        #[arg(short = 's', long)]
        schedule: String,

        /// Docker image to run (e.g. alpine, curlimages/curl)
        #[arg(short = 'i', long)]
        image: String,

        /// Command to run in the container (space-separated, e.g. "curl https://example.com")
        #[arg(short = 'c', long)]
        command: Option<String>,

        /// Project ID to associate with (optional)
        #[arg(long)]
        project: Option<String>,
    },

    /// Show details for a cron job
    Info {
        /// Cron job name or ID
        name: String,
    },

    /// Update a cron job's schedule, image, command, or enabled state
    Update {
        /// Cron job name or ID
        name: String,

        /// New cron schedule (5-field)
        #[arg(short = 's', long)]
        schedule: Option<String>,

        /// New Docker image
        #[arg(short = 'i', long)]
        image: Option<String>,

        /// New command (space-separated)
        #[arg(short = 'c', long)]
        command: Option<String>,

        /// Enable the job (pass --enable or --no-enable)
        #[arg(long)]
        enable: Option<bool>,
    },

    /// Delete a cron job
    Delete {
        /// Cron job name or ID
        name: String,

        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Trigger a cron job immediately (manual run)
    Trigger {
        /// Cron job name or ID
        name: String,
    },

    /// Show run history for a cron job
    Runs {
        /// Cron job name or ID
        name: String,
    },
}

pub async fn handle(client: &TondClient, action: CronAction) -> Result<()> {
    match action {
        CronAction::List => {
            let jobs = client.list_cron_jobs().await?;
            if jobs.is_empty() {
                println!("No cron jobs found.");
            } else {
                println!(
                    "{:<24} {:<20} {:<22} {:<8} ENABLED",
                    "NAME", "SCHEDULE", "NEXT RUN", "STATUS"
                );
                println!("{}", "-".repeat(90));
                for job in &jobs {
                    println!(
                        "{:<24} {:<20} {:<22} {:<8} {}",
                        job.name,
                        job.schedule,
                        job.next_run_at.as_deref().unwrap_or("-"),
                        job.status,
                        if job.enabled { "yes" } else { "no" }
                    );
                }
            }
        }

        CronAction::Create {
            name,
            schedule,
            image,
            command,
            project,
        } => {
            let cmd_vec: Option<Vec<String>> = command
                .as_ref()
                .map(|c| c.split_whitespace().map(String::from).collect());
            let job = client
                .create_cron_job(
                    &name,
                    &schedule,
                    &image,
                    cmd_vec.as_deref(),
                    project.as_deref(),
                )
                .await?;
            println!("Cron job created: {} (ID: {})", job.name, job.id);
            println!("Schedule:  {}", job.schedule);
            println!("Image:     {}", job.image);
            println!("Next run:  {}", job.next_run_at.as_deref().unwrap_or("-"));
        }

        CronAction::Info { name } => {
            let job = client.get_cron_job(&name).await?;
            println!("Name:      {}", job.name);
            println!("ID:        {}", job.id);
            println!("Schedule:  {}", job.schedule);
            println!("Image:     {}", job.image);
            println!("Command:   {}", job.command.as_deref().unwrap_or("-"));
            println!("Project:   {}", job.project_id.as_deref().unwrap_or("-"));
            println!("Enabled:   {}", if job.enabled { "yes" } else { "no" });
            println!("Status:    {}", job.status);
            println!("Last run:  {}", job.last_run_at.as_deref().unwrap_or("-"));
            println!("Next run:  {}", job.next_run_at.as_deref().unwrap_or("-"));
            println!("Created:   {}", job.created_at);
        }

        CronAction::Update {
            name,
            schedule,
            image,
            command,
            enable,
        } => {
            let cmd_vec: Option<Vec<String>> = command
                .as_ref()
                .map(|c| c.split_whitespace().map(String::from).collect());
            let job = client
                .update_cron_job(
                    &name,
                    schedule.as_deref(),
                    image.as_deref(),
                    cmd_vec.as_deref(),
                    enable,
                )
                .await?;
            println!("Cron job updated: {}", job.name);
            println!("Schedule:  {}", job.schedule);
            println!("Enabled:   {}", if job.enabled { "yes" } else { "no" });
            println!("Next run:  {}", job.next_run_at.as_deref().unwrap_or("-"));
        }

        CronAction::Delete { name, yes } => {
            if !yes {
                print!("Are you sure you want to delete cron job {}? [y/N] ", name);
                use std::io::{self, Write};
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() != "y" {
                    println!("Cancelled");
                    return Ok(());
                }
            }
            client.delete_cron_job(&name).await?;
            println!("Cron job {} deleted", name);
        }

        CronAction::Trigger { name } => {
            client.trigger_cron_job(&name).await?;
            println!("Cron job {} triggered", name);
        }

        CronAction::Runs { name } => {
            let runs = client.list_cron_job_runs(&name).await?;
            if runs.is_empty() {
                println!("No runs found for cron job '{}'.", name);
            } else {
                println!("{:<36} {:<10} {:<6} STARTED AT", "RUN ID", "STATUS", "EXIT");
                println!("{}", "-".repeat(80));
                for run in &runs {
                    println!(
                        "{:<36} {:<10} {:<6} {}",
                        run.id,
                        run.status,
                        run.exit_code
                            .map(|c| c.to_string())
                            .as_deref()
                            .unwrap_or("-"),
                        run.started_at
                    );
                }
            }
        }
    }
    Ok(())
}
