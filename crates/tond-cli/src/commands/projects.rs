// commands/projects.rs — `tond projects` subcommand definitions and handler

use anyhow::Result;
use clap::Subcommand;

use crate::client::TondClient;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum ProjectsAction {
    /// List all projects
    #[command(visible_alias = "ls")]
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
    #[command(visible_aliases = ["show", "get"])]
    Info {
        /// Project name or ID
        project: String,
    },

    /// Delete a project (apps are un-assigned, not deleted; asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Project name or ID
        project: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
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
    #[command(visible_alias = "previews")]
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
    #[command(visible_alias = "ls")]
    List {
        /// Project name or ID
        project: String,
    },

    /// Delete (stop + expire) a project preview (asks first)
    #[command(visible_alias = "rm")]
    Delete {
        /// Project name or ID
        project: String,

        /// Project preview ID
        preview_id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

pub async fn handle(client: &TondClient, out: &Output, action: ProjectsAction) -> Result<()> {
    match action {
        ProjectsAction::List => {
            let projects = client.list_projects().await?;
            out.json(&projects)?;
            if projects.is_empty() {
                outln!(
                    out,
                    "No projects yet. Create one with: tond projects create <name>"
                );
                return Ok(());
            }
            outln!(
                out,
                "{:<28} {:<30} {:<8} {:<26}",
                "ID",
                "NAME",
                "PREVIEWS",
                "CREATED"
            );
            outln!(out, "{}", "-".repeat(92));
            for p in &projects {
                outln!(
                    out,
                    "{:<28} {:<30} {:<8} {:<26}",
                    p.id,
                    p.name,
                    p.active_preview_count,
                    p.created_at
                );
            }
        }

        ProjectsAction::Create { name, description } => {
            let p = client.create_project(&name, description.as_deref()).await?;
            out.json(&p)?;
            outln!(out, "Project created: {} (ID: {})", p.name, p.id);
        }

        ProjectsAction::Info { project } => {
            let p = client.get_project(&project).await?;
            out.json(&p)?;
            outln!(out, "Name:        {}", p.name);
            outln!(out, "ID:          {}", p.id);
            outln!(
                out,
                "Description: {}",
                p.description.as_deref().unwrap_or("-")
            );
            outln!(out, "Created:     {}", p.created_at);
            outln!(out, "Updated:     {}", p.updated_at);
        }

        ProjectsAction::Delete { project, yes } => {
            out.confirm(
                yes,
                &format!("Delete project {project}? Its apps are un-assigned, not deleted."),
                &format!("delete project {project}"),
            )?;
            let data = client.delete_project(&project).await?;
            out.done("delete", "project", &project, data)?;
            outln!(out, "Project {} deleted", project);
        }

        ProjectsAction::Deploy { project } => {
            outln!(out, "Deploying all apps in project {}...", project);
            let data = client.deploy_project(&project).await?;
            out.done("deploy", "project", &project, data)?;
            outln!(
                out,
                "Deployment queued! Poll each app with: tond apps info <app>"
            );
        }

        ProjectsAction::Assign { app, project } => {
            let data = client.assign_app_project(&app, project.as_deref()).await?;
            out.done("assign_project", "app", &app, data)?;
            match project {
                Some(p) => outln!(out, "App {} assigned to project {}", app, p),
                None => outln!(out, "App {} removed from its project", app),
            }
        }

        ProjectsAction::Preview { action } => match action {
            PreviewAction::Create { project, ttl } => {
                outln!(
                    out,
                    "Creating project preview for {} (TTL: {}h)...",
                    project,
                    ttl
                );
                let pp = client.create_project_preview(&project, Some(ttl)).await?;
                out.json(&pp)?;
                outln!(out, "Project preview created:");
                outln!(out, "  ID:      {}", pp.id);
                outln!(out, "  Slug:    {}", pp.slug);
                outln!(out, "  Status:  {}", pp.status);
                outln!(out, "  Expires: {}", pp.expires_at);
                if !pp.previews.is_empty() {
                    outln!(out, "  App previews:");
                    for p in &pp.previews {
                        let url = p.url.as_deref().unwrap_or("-");
                        outln!(out, "    {} → {} ({})", p.app_id, url, p.status);
                    }
                }
                outln!(
                    out,
                    "\nPoll status with: tond projects preview list {}",
                    project
                );
            }

            PreviewAction::List { project } => {
                let previews = client.list_project_previews(&project).await?;
                out.json(&previews)?;
                if previews.is_empty() {
                    outln!(out, "No active project previews for {}", project);
                    return Ok(());
                }
                outln!(
                    out,
                    "{:<28} {:<14} {:<16} {:<26}",
                    "ID",
                    "SLUG",
                    "STATUS",
                    "EXPIRES"
                );
                outln!(out, "{}", "-".repeat(84));
                for pp in &previews {
                    outln!(
                        out,
                        "{:<28} {:<14} {:<16} {:<26}",
                        pp.id,
                        pp.slug,
                        pp.status,
                        pp.expires_at
                    );
                    for p in &pp.previews {
                        let url = p.url.as_deref().unwrap_or("-");
                        outln!(out, "  ↳ app:{:<24} {} ({})", p.app_id, url, p.status);
                    }
                }
            }

            PreviewAction::Delete {
                project,
                preview_id,
                yes,
            } => {
                out.confirm(
                    yes,
                    &format!(
                        "Delete project preview {preview_id}? All its containers are stopped."
                    ),
                    &format!("delete project preview {preview_id}"),
                )?;
                let data = client.delete_project_preview(&project, &preview_id).await?;
                out.done("delete", "project_preview", &preview_id, data)?;
                outln!(out, "Project preview {} deleted", preview_id);
            }
        },
    }
    Ok(())
}
