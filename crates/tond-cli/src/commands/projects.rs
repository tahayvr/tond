// commands/projects.rs — `tond projects` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;

#[derive(Subcommand)]
pub enum ProjectsAction {
    /// List all projects
    List,

    /// Create a new project
    Create {
        /// Project name
        name: String,

        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },

    /// Show project details and member apps
    Info {
        /// Project name or ID
        project: String,
    },

    /// Delete a project (apps are un-assigned, not deleted)
    Delete {
        /// Project name or ID
        project: String,

        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Deploy all apps in a project
    Deploy {
        /// Project name or ID
        project: String,
    },

    /// Assign an app to a project (or remove it)
    Assign {
        /// App name or ID
        app: String,

        /// Project name or ID to assign (omit to remove from project)
        #[arg(long)]
        project: Option<String>,
    },

    /// Manage project preview environments
    Preview {
        #[command(subcommand)]
        action: PreviewAction,
    },
}

#[derive(Subcommand)]
pub enum PreviewAction {
    /// Create a project preview (one preview per member app, shared slug)
    Create {
        /// Project name or ID
        project: String,

        /// Time-to-live in hours (default 24, max 168)
        #[arg(long, default_value = "24")]
        ttl: i64,
    },

    /// List active project previews
    List {
        /// Project name or ID
        project: String,
    },

    /// Delete (stop + expire) a project preview
    Delete {
        /// Project name or ID
        project: String,

        /// Project preview ID
        preview_id: String,

        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

pub async fn handle(client: &TondClient, action: ProjectsAction) -> Result<()> {
    match action {
        ProjectsAction::List => {
            let projects = client.list_projects().await?;
            if projects.is_empty() {
                println!("No projects yet. Create one with: tond projects create <name>");
                return Ok(());
            }
            println!(
                "{:<28} {:<30} {:<8} {:<26}",
                "ID", "NAME", "PREVIEWS", "CREATED"
            );
            println!("{}", "-".repeat(92));
            for p in &projects {
                println!(
                    "{:<28} {:<30} {:<8} {:<26}",
                    p.id, p.name, p.active_preview_count, p.created_at
                );
            }
        }

        ProjectsAction::Create { name, description } => {
            let p = client.create_project(&name, description.as_deref()).await?;
            println!("Project created: {} (ID: {})", p.name, p.id);
        }

        ProjectsAction::Info { project } => {
            let p = client.get_project(&project).await?;
            println!("Name:        {}", p.name);
            println!("ID:          {}", p.id);
            println!("Description: {}", p.description.as_deref().unwrap_or("-"));
            println!("Created:     {}", p.created_at);
            println!("Updated:     {}", p.updated_at);
        }

        ProjectsAction::Delete { project, yes } => {
            if !yes {
                print!(
                    "Delete project {}? Apps will be un-assigned. [y/N] ",
                    project
                );
                use std::io::{self, Write};
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() != "y" {
                    println!("Cancelled");
                    return Ok(());
                }
            }
            client.delete_project(&project).await?;
            println!("Project {} deleted", project);
        }

        ProjectsAction::Deploy { project } => {
            println!("Deploying all apps in project {}...", project);
            client.deploy_project(&project).await?;
            println!("Deployment queued! Poll each app with: tond apps info <app>");
        }

        ProjectsAction::Assign { app, project } => {
            client.assign_app_project(&app, project.as_deref()).await?;
            match project {
                Some(p) => println!("App {} assigned to project {}", app, p),
                None => println!("App {} removed from its project", app),
            }
        }

        ProjectsAction::Preview { action } => match action {
            PreviewAction::Create { project, ttl } => {
                println!(
                    "Creating project preview for {} (TTL: {}h)...",
                    project, ttl
                );
                let pp = client.create_project_preview(&project, Some(ttl)).await?;
                println!("Project preview created:");
                println!("  ID:      {}", pp.id);
                println!("  Slug:    {}", pp.slug);
                println!("  Status:  {}", pp.status);
                println!("  Expires: {}", pp.expires_at);
                if !pp.previews.is_empty() {
                    println!("  App previews:");
                    for p in &pp.previews {
                        let url = p.url.as_deref().unwrap_or("-");
                        println!("    {} → {} ({})", p.app_id, url, p.status);
                    }
                }
                println!("\nPoll status with: tond projects preview list {}", project);
            }

            PreviewAction::List { project } => {
                let previews = client.list_project_previews(&project).await?;
                if previews.is_empty() {
                    println!("No active project previews for {}", project);
                    return Ok(());
                }
                println!(
                    "{:<28} {:<14} {:<16} {:<26}",
                    "ID", "SLUG", "STATUS", "EXPIRES"
                );
                println!("{}", "-".repeat(84));
                for pp in &previews {
                    println!(
                        "{:<28} {:<14} {:<16} {:<26}",
                        pp.id, pp.slug, pp.status, pp.expires_at
                    );
                    for p in &pp.previews {
                        let url = p.url.as_deref().unwrap_or("-");
                        println!("  ↳ app:{:<24} {} ({})", p.app_id, url, p.status);
                    }
                }
            }

            PreviewAction::Delete {
                project,
                preview_id,
                yes,
            } => {
                if !yes {
                    print!(
                        "Delete project preview {}? This will stop all child containers. [y/N] ",
                        preview_id
                    );
                    use std::io::{self, Write};
                    io::stdout().flush()?;
                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    if input.trim().to_lowercase() != "y" {
                        println!("Cancelled");
                        return Ok(());
                    }
                }
                client.delete_project_preview(&project, &preview_id).await?;
                println!("Project preview {} deleted", preview_id);
            }
        },
    }
    Ok(())
}
