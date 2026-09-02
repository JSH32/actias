//! `actias project`: listing the projects this account can see, and
//! creating or deleting one.

use colored::*;
use prettytable::{Table, row};

use crate::{
    client::{Client, types::CreateProjectDto},
    commands::ProjectOperations,
    errors::{Error, Result, progenitor_error},
    ui,
};

/// Prints one page of the projects this account can see.
///
/// # Errors
/// Returns the api's message.
pub async fn handle_list(client: &Client, page: f64) -> Result<()> {
    let response = client
        .list_projects()
        .page(page)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    let mut table = Table::new();
    table.add_row(row!["ID", "Name", "Created At", "Updated At"]);

    println!(
        "page {} of {}",
        response.page.to_string().yellow(),
        response.last_page.to_string().yellow()
    );

    for item in response.items {
        table.add_row(row![item.id, item.name, item.created_at, item.updated_at]);
    }

    table.printstd();

    Ok(())
}

/// Runs one `actias project` subcommand.
///
/// # Errors
/// Returns the api's message, or the prompt's when the operator cancels.
pub async fn handle_operation(client: &Client, operation: &ProjectOperations) -> Result<()> {
    match operation {
        ProjectOperations::Create { name } => {
            // The generated client enforces this too, but as a failed
            // body conversion, which reads like a bug rather than a
            // typo.
            if !(6..=36).contains(&name.chars().count()) {
                return Err(Error::Command(
                    "a project name is 6 to 36 characters".to_owned(),
                ));
            }

            let project = client
                .create_project()
                .body(CreateProjectDto::builder().name(name.clone()))
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done(
                "Created",
                format!(
                    "project {} {}",
                    project.name,
                    format!("({})", project.id).bright_black()
                ),
            );
            ui::detail(format!("actias init my-app basic {}", project.id).bright_black());
        }
        ProjectOperations::Delete { id } => {
            // Read first, so the confirmation names what went rather than
            // echoing back the id that was typed.
            let project = client
                .get_project()
                .project(id)
                .send()
                .await
                .map_err(progenitor_error)?;

            client
                .delete_project()
                .project(id)
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done(
                "Deleted",
                format!(
                    "project {} {}",
                    project.name,
                    format!("({})", project.id).bright_black()
                ),
            );
        }
    }

    Ok(())
}
