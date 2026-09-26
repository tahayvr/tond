// commands/cron.rs — `tond cron` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum CronAction {
    /// List all cron jobs
    #[command(visible_alias = "ls")]
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
    #[command(visible_aliases = ["show", "get"])]
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

    /// Delete a cron job (asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Cron job name or ID
        name: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Trigger a cron job immediately (manual run)
    #[command(visible_alias = "run")]
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

pub async fn handle(client: &TondClient, out: &Output, action: CronAction) -> Result<()> {
    match action {
        CronAction::List => {
            let jobs = client.list_cron_jobs().await?;
            out.json(&jobs)?;
            if jobs.is_empty() {
                outln!(out, "No cron jobs found.");
            } else {
                outln!(
                    out,
                    "{:<24} {:<20} {:<22} {:<8} ENABLED",
                    "NAME",
                    "SCHEDULE",
                    "NEXT RUN",
                    "STATUS"
                );
                outln!(out, "{}", "-".repeat(90));
                for job in &jobs {
                    outln!(
                        out,
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
            out.json(&job)?;
            outln!(out, "Cron job created: {} (ID: {})", job.name, job.id);
            outln!(out, "Schedule:  {}", job.schedule);
            outln!(out, "Image:     {}", job.image);
            outln!(
                out,
                "Next run:  {}",
                job.next_run_at.as_deref().unwrap_or("-")
            );
        }

        CronAction::Info { name } => {
            let job = client.get_cron_job(&name).await?;
            out.json(&job)?;
            outln!(out, "Name:      {}", job.name);
            outln!(out, "ID:        {}", job.id);
            outln!(out, "Schedule:  {}", job.schedule);
            outln!(out, "Image:     {}", job.image);
            outln!(out, "Command:   {}", job.command.as_deref().unwrap_or("-"));
            outln!(
                out,
                "Project:   {}",
                job.project_id.as_deref().unwrap_or("-")
            );
            outln!(out, "Enabled:   {}", if job.enabled { "yes" } else { "no" });
            outln!(out, "Status:    {}", job.status);
            outln!(
                out,
                "Last run:  {}",
                job.last_run_at.as_deref().unwrap_or("-")
            );
            outln!(
                out,
                "Next run:  {}",
                job.next_run_at.as_deref().unwrap_or("-")
            );
            outln!(out, "Created:   {}", job.created_at);
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
            out.json(&job)?;
            outln!(out, "Cron job updated: {}", job.name);
            outln!(out, "Schedule:  {}", job.schedule);
            outln!(out, "Enabled:   {}", if job.enabled { "yes" } else { "no" });
            outln!(
                out,
                "Next run:  {}",
                job.next_run_at.as_deref().unwrap_or("-")
            );
        }

        CronAction::Delete { name, yes } => {
            out.confirm(
                yes,
                &format!("Delete cron job {name} and its run history?"),
                &format!("delete cron job {name}"),
            )?;
            let data = client.delete_cron_job(&name).await?;
            out.done("delete", "cron_job", &name, data)?;
            outln!(out, "Cron job {} deleted", name);
        }

        CronAction::Trigger { name } => {
            let data = client.trigger_cron_job(&name).await?;
            out.done("trigger", "cron_job", &name, data)?;
            outln!(out, "Cron job {} triggered", name);
        }

        CronAction::Runs { name } => {
            let runs = client.list_cron_job_runs(&name).await?;
            out.json(&runs)?;
            if runs.is_empty() {
                outln!(out, "No runs found for cron job '{}'.", name);
            } else {
                outln!(
                    out,
                    "{:<36} {:<10} {:<6} STARTED AT",
                    "RUN ID",
                    "STATUS",
                    "EXIT"
                );
                outln!(out, "{}", "-".repeat(80));
                for run in &runs {
                    outln!(
                        out,
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
