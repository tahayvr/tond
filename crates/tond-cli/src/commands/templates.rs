// commands/templates.rs — `tond templates`: browse the one-click catalog
//
// Deploying one is `tond apps create <name> --template <id>`.
//
// JSON output (--json): `list` prints the catalog response ({templates,
// memory, apps_domain}), `show` prints one template.

use anyhow::Result;
use clap::Subcommand;
use tond_common::{TemplateCatalogResponse, TemplateResponse};

use crate::client::TondClient;
use crate::error::CliError;
use crate::output::{Output, out};

#[derive(Subcommand)]
pub enum TemplatesAction {
    /// List the templates, with their memory needs
    #[command(visible_alias = "ls")]
    List,
    /// Show a template's services, inputs and notes
    #[command(visible_aliases = ["info", "get"])]
    Show {
        /// Template id (see `tond templates list`)
        id: String,
    },
}

pub async fn handle(client: &TondClient, output: &Output, action: TemplatesAction) -> Result<()> {
    let catalog = client.list_templates().await?;
    match action {
        TemplatesAction::List => {
            output.json(&catalog)?;
            out!(output, "{}", format_list(&catalog));
        }
        TemplatesAction::Show { id } => {
            let template = catalog
                .templates
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| {
                    CliError::usage(format!("Unknown template '{id}'."))
                        .with_hint("See `tond templates list`.")
                })?;
            output.json(template)?;
            out!(output, "{}", format_details(template));
        }
    }
    Ok(())
}

fn format_ram(mb: u32) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", f64::from(mb) / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

fn format_list(catalog: &TemplateCatalogResponse) -> String {
    let mut out = format!(
        "{:<20} {:<24} {:<14} {:>8}  {}\n",
        "ID", "NAME", "CATEGORY", "RAM", "FITS 2 GB"
    );
    for t in &catalog.templates {
        out.push_str(&format!(
            "{:<20} {:<24} {:<14} {:>8}  {}\n",
            t.id,
            t.name,
            t.category,
            format!("~{}", format_ram(t.ram_mb)),
            if t.fits_2gb { "yes" } else { "no" }
        ));
    }
    if let Some(memory) = catalog.memory {
        out.push_str(&format!(
            "\nThis server has {} of {} memory free.\n",
            format_ram(memory.available_mb as u32),
            format_ram(memory.total_mb as u32)
        ));
    }
    out.push_str("Deploy one with: tond apps create <name> --template <id>\n");
    out
}

fn format_details(t: &TemplateResponse) -> String {
    let mut out = format!(
        "{} {}\n{}\n\nNeeds ~{} of memory{}.\n",
        t.name,
        t.version,
        t.description,
        format_ram(t.ram_mb),
        if t.fits_2gb {
            ", fits on a 2 GB server"
        } else {
            ""
        }
    );
    out.push_str("\nServices:\n");
    for s in &t.services {
        out.push_str(&format!(
            "  {:<16} {}{}\n",
            s.name,
            s.image,
            if s.public { "  (public)" } else { "" }
        ));
    }
    if !t.inputs.is_empty() {
        out.push_str("\nInputs (--env KEY=VALUE):\n");
        for i in &t.inputs {
            let note = if i.generated {
                "generated if empty"
            } else if i.default.is_some() {
                "optional"
            } else if i.required {
                "required"
            } else {
                "optional"
            };
            out.push_str(&format!("  {:<24} {} ({note})\n", i.key, i.label));
        }
    }
    if let Some(notes) = &t.setup_notes {
        out.push_str(&format!("\nAfter deploying: {notes}\n"));
    }
    if let Some(docs) = &t.docs {
        out.push_str(&format!("Docs: {docs}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tond_common::builtin_templates;

    fn catalog() -> TemplateCatalogResponse {
        TemplateCatalogResponse {
            templates: builtin_templates()
                .unwrap_or_else(|e| panic!("{e}"))
                .into_iter()
                .map(TemplateResponse::from)
                .collect(),
            memory: Some(tond_common::TemplateHostMemory {
                total_mb: 2048,
                available_mb: 1536,
            }),
            apps_domain: String::new(),
        }
    }

    #[test]
    fn list_shows_ram_and_free_memory() {
        let out = format_list(&catalog());
        assert!(out.contains("umami"), "{out}");
        assert!(out.contains("1.5 GB of 2.0 GB memory free"), "{out}");
    }

    #[test]
    fn details_show_services_and_inputs() {
        let c = catalog();
        let t = c
            .templates
            .iter()
            .find(|t| t.id == "pocketbase")
            .unwrap_or_else(|| panic!("pocketbase"));
        let out = format_details(t);
        assert!(out.contains("(public)"), "{out}");
        assert!(out.contains("SUPERUSER_EMAIL"), "{out}");
        assert!(out.contains("generated if empty"), "{out}");
    }
}
