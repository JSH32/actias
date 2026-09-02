//! `actias script revision`: listing revisions, pointing a script at
//! one, cloning one back to disk, and deleting one.

use colored::*;
use include_dir::{Dir, include_dir};
use inquire::{Confirm, Select};
use prettytable::{Table, row};
use std::path::PathBuf;

use crate::{
    client::{Client, types::ScriptDto},
    errors::{Error, Result, progenitor_error},
    handlers::publish,
    script::ScriptConfig,
    ui,
    util::{copy_definitions, copy_dir_all, get_dir, write_revision},
};

// Reference to template directory
static PROJ_TEMPLATE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template/templates");

/// Deletes one revision, moving the script's pointer when it was current.
///
/// # Errors
/// Returns the api's message, or the prompt's when the operator cancels.
pub async fn handle_delete(client: &Client, script: &ScriptDto, revision_id: &str) -> Result<()> {
    let result = client
        .delete_revision()
        .id(revision_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    ui::done("Deleted", format!("revision {revision_id}"));
    ui::done(
        "Set",
        format!(
            "{}'s {} revision to {}",
            script.public_identifier,
            format!("({})", result.script_id).bright_black(),
            match &result.revision_id {
                Some(v) => v.yellow(),
                None => "NONE".red(),
            }
        ),
    );

    Ok(())
}

/// Prints one page of a script's revisions.
///
/// # Errors
/// Returns the api's message.
pub async fn handle_list(client: &Client, script_id: &str, page: &Option<i64>) -> Result<()> {
    let script = client
        .get_script()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    let response = client
        .revision_list()
        .id(script_id)
        .page(page.unwrap_or(1) as f64)
        .send()
        .await
        .map_err(progenitor_error)?;

    let mut table = Table::new();
    table.add_row(row!["ID", "Created",]);

    println!(
        "page {} of {}",
        response.page.to_string().yellow(),
        response.last_page.to_string().yellow()
    );

    for item in &response.items {
        table.add_row(row![
            if item.id == script.current_revision_id.clone().unwrap_or("".to_string()) {
                item.id.green()
            } else {
                item.id.white()
            },
            item.created
        ]);
    }

    table.printstd();

    Ok(())
}

/// Points the script at one revision.
///
/// # Errors
/// Returns the api's message.
pub async fn handle_set(client: &Client, script_id: &str, revision_id: &str) -> Result<()> {
    let script = client
        .get_script()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    let response = client
        .set_revision()
        .id(script_id)
        .revision_id(revision_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    ui::done(
        "Set",
        format!(
            "{}'s {} revision to {}",
            script.public_identifier,
            format!("({})", response.script_id).bright_black(),
            response.revision_id.clone().unwrap().yellow()
        ),
    );

    Ok(())
}

/// Writes one revision's files into a local directory.
pub async fn handle_clone(
    client: &Client,
    script: &ScriptDto,
    revision_id: &str,
    path: Option<String>,
) -> Result<()> {
    let revision = client
        .get_revision()
        .id(revision_id)
        .with_bundle(true)
        .send()
        .await
        .map_err(progenitor_error)?;

    let mut script_path = std::env::current_dir().unwrap();
    script_path.push(script.public_identifier.clone());

    let path = path.map(PathBuf::from).unwrap_or(script_path);

    write_revision(path.clone(), revision.clone()).map_err(Error::Io)?;

    copy_definitions(&path).map_err(Error::Script)?;

    println!(
        "revision {} for {} {}",
        format!("({})", revision.id).bright_black(),
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );

    Ok(())
}

/// Publishes the built-in sample as a script's first revision.
///
/// # Errors
/// Returns the api's message, or the filesystem's when the sample cannot
/// be written out.
pub async fn handle_create_sample(client: &Client, script: &ScriptDto) -> Result<()> {
    println!("Script does not have a current revision");
    if !Confirm::new("Do you want to create one?")
        .with_default(true)
        .with_help_message("This will create a sample revision")
        .prompt()
        .map_err(|e| Error::Command(e.to_string()))?
    {
        return Ok(());
    }

    let script_path = get_dir(&script.public_identifier, true, true).map_err(Error::Io)?;

    let template_names: Vec<String> = PROJ_TEMPLATE_DIR
        .dirs()
        .map(|f| f.path().file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    let template = Select::new("What template would you like to use?", template_names)
        .prompt()
        .map_err(|e| Error::Command(e.to_string()))?;

    // This is a workaround that creates the template dir and then copies it out and deletes the temp dir.
    {
        let mut temp_path = script_path.clone();
        temp_path.push(&template);

        std::fs::create_dir_all(temp_path.clone())
            .map_err(|e| Error::Io(format!("Failed to create directory: {}", e)))?;

        PROJ_TEMPLATE_DIR
            .get_dir(&template)
            .unwrap()
            .extract(&script_path)
            .map_err(|e| Error::Io(format!("Failed to extract template: {}", e)))?;

        copy_dir_all(&temp_path, &script_path)
            .map_err(|e| Error::Io(format!("Failed to copy files: {}", e)))?;

        std::fs::remove_dir_all(temp_path)
            .map_err(|e| Error::Io(format!("Failed to clean up: {}", e)))?;
    }

    let mut script_config = ScriptConfig::from_path(&script_path).map_err(Error::Script)?;

    script_config.id = Some(script.id.clone());
    script_config
        .write_config(&script_path)
        .map_err(Error::Io)?;

    copy_definitions(&script_path).map_err(Error::Script)?;

    publish::handle(client, script_path.to_str().unwrap()).await
}
