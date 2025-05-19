use colored::*;
use prettytable::{Table, row};

use crate::{
    client::Client,
    commands::ProjectOperations,
    errors::{Result, progenitor_error},
};

/// Handle listing projects
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
        "🔍 Displaying project page {} of {}",
        response.page.to_string().yellow(),
        response.last_page.to_string().yellow()
    );

    for item in response.items {
        table.add_row(row![item.id, item.name, item.created_at, item.updated_at]);
    }

    table.printstd();

    Ok(())
}

/// Handle project operations
pub async fn handle_operation(
    client: &Client,
    id: &str,
    operation: &ProjectOperations,
) -> Result<()> {
    let project = client
        .get_project()
        .project(id)
        .send()
        .await
        .map_err(progenitor_error)?;

    match operation {
        ProjectOperations::Delete => {
            client
                .delete_project()
                .project(id)
                .send()
                .await
                .map_err(progenitor_error)?;

            println!(
                "🚮 Deleted project {} {}",
                project.name.purple(),
                format!("({})", project.id).bright_black()
            );
        }
    }

    Ok(())
}
