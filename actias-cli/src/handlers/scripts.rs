use colored::*;
use prettytable::{Table, row};
use std::path::Path;

use crate::{
    client::Client,
    commands::{RevisionCommands, ScriptOperations},
    errors::{Error, Result, progenitor_error},
    handlers::revisions,
    script::ScriptConfig,
};

/// Handle listing scripts
pub async fn handle_list(client: &Client, project: &str, page: f64) -> Result<()> {
    let response = client
        .list_scripts()
        .project(project)
        .page(page)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    let mut table = Table::new();
    table.add_row(row![
        "ID",
        "Public Identifier",
        "Last Updated",
        "Current Revision"
    ]);

    println!(
        "🔍 Displaying script page {} of {}",
        response.page.to_string().yellow(),
        response.last_page.to_string().yellow()
    );

    for item in response.items {
        table.add_row(row![
            item.id,
            item.public_identifier,
            item.last_updated,
            item.current_revision_id
                .clone()
                .unwrap_or("No Revision!".yellow().to_string())
        ]);
    }

    table.printstd();

    Ok(())
}

/// Handle script operations
pub async fn handle_operation(
    client: &Client,
    script_id: &str,
    operation: &ScriptOperations,
) -> Result<()> {
    let path = Path::new(script_id);

    // Resolve script ID - either from config or use directly
    let id = match ScriptConfig::from_path(&path) {
        Ok(v) => v.id.unwrap_or(script_id.to_string()),
        Err(_) => script_id.to_string(),
    };

    match operation {
        ScriptOperations::Delete => handle_delete(client, &id).await,
        ScriptOperations::Clone { path } => handle_clone(client, &id, path.clone()).await,
        ScriptOperations::Revisions { sub } => handle_revisions(client, &id, sub).await,
    }
}

/// Handle script deletion
async fn handle_delete(client: &Client, id: &str) -> Result<()> {
    let script = client
        .get_script()
        .id(id)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    client
        .delete_script()
        .id(id)
        .send()
        .await
        .map_err(progenitor_error)?;

    println!(
        "🚮 Deleted script {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black()
    );

    Ok(())
}

/// Handle script cloning
async fn handle_clone(client: &Client, id: &str, path: Option<String>) -> Result<()> {
    let script = client
        .get_script()
        .id(id)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    match &script.current_revision_id {
        Some(v) => revisions::handle_clone(client, &script, v, path).await,
        None => revisions::handle_create_sample(client, &script).await,
    }
}

/// Handle revision operations
async fn handle_revisions(
    client: &Client,
    script_id: &str,
    command: &RevisionCommands,
) -> Result<()> {
    let script = client
        .get_script()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    match command {
        RevisionCommands::Delete { revision_id } => {
            revisions::handle_delete(client, &script, revision_id).await
        }
        RevisionCommands::List { page } => revisions::handle_list(client, script_id, page).await,
        RevisionCommands::Set { revision_id } => {
            revisions::handle_set(client, script_id, revision_id).await
        }
        RevisionCommands::Clone { revision_id, path } => {
            let revision_id = revision_id.clone().unwrap_or(
                script
                    .clone()
                    .current_revision_id
                    .ok_or(Error::NotFound("No current revision set".to_string()))?,
            );

            revisions::handle_clone(client, &script, &revision_id, path.clone()).await
        }
    }
}
