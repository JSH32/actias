use colored::*;
use include_dir::{Dir, include_dir};
use inquire::Select;
use std::path::Path;

use crate::{
    client::{Client, types::CreateRevisionDto, types::CreateScriptDto},
    errors::{Error, Result, progenitor_error},
    script::ScriptConfig,
    util::{copy_definitions, copy_dir_all, get_dir},
};

// Reference to template directory
static PROJ_TEMPLATE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template/templates");

/// Handle the Init command
pub async fn handle(
    client: &Client,
    script_name: &str,
    project_id: Option<String>,
    template: Option<String>,
) -> Result<()> {
    // Ensure user has access if project ID is provided
    if let Some(project_id) = &project_id {
        check_script_permission(client, project_id).await?;
    }

    // Select or validate template
    let (template_name, template_dir) = select_template(template)?;

    // Create script directory
    let script_path = get_dir(script_name, true, true).map_err(|e| Error::Io(e))?;

    // Extract template to script directory
    extract_template(&script_path, &template_name, template_dir)?;

    // Generate TypeScript definitions
    copy_definitions(&script_path).map_err(|e| Error::Script(e))?;

    println!(
        "📜 Script {} was created!",
        script_path.file_name().unwrap().to_str().unwrap().magenta()
    );

    // Create script in API if project ID was provided
    if let Some(project_id) = project_id {
        publish_new_script(client, &script_path, script_name, &project_id).await?;
    }

    Ok(())
}

/// Check if the user has script write permission for the project
async fn check_script_permission(client: &Client, project_id: &str) -> Result<()> {
    let acl = client
        .get_acl_me()
        .project(project_id)
        .send()
        .await
        .map_err(|e| Error::Api(e.to_string()))?;

    if !acl.permissions.contains_key("SCRIPT_WRITE") {
        return Err(Error::Permission(
            "No permission to write scripts".to_string(),
        ));
    }

    Ok(())
}

/// Select template from available options or validate provided template
fn select_template(template: Option<String>) -> Result<(String, &'static Dir<'static>)> {
    let names: Vec<String> = PROJ_TEMPLATE_DIR
        .dirs()
        .map(|f| f.path().file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    match template {
        Some(template) => {
            if !names.contains(&template) {
                return Err(Error::NotFound(format!(
                    "Template {} not found, available templates: {:#?}",
                    template, names
                )));
            }
            Ok((
                template.clone(),
                PROJ_TEMPLATE_DIR.get_dir(&template).unwrap(),
            ))
        }
        None => {
            let selected = Select::new("What template would you like to use?", names)
                .prompt()
                .map_err(|e| Error::Command(e.to_string()))?;

            Ok((
                selected.clone(),
                PROJ_TEMPLATE_DIR.get_dir(&selected).unwrap(),
            ))
        }
    }
}

/// Extract template to script directory
fn extract_template(script_path: &Path, template_name: &str, template: &Dir<'_>) -> Result<()> {
    // This is a workaround that creates the template dir and then copies it out and deletes the temp dir
    let mut temp_path = script_path.to_path_buf();
    temp_path.push(template_name);

    std::fs::create_dir_all(&temp_path)
        .map_err(|e| Error::Io(format!("Failed to create temp directory: {}", e)))?;

    template
        .extract(script_path)
        .map_err(|e| Error::Io(format!("Failed to extract template: {}", e)))?;

    copy_dir_all(&temp_path, script_path)
        .map_err(|e| Error::Io(format!("Failed to copy files: {}", e)))?;

    std::fs::remove_dir_all(temp_path)
        .map_err(|e| Error::Io(format!("Failed to clean up temp directory: {}", e)))?;

    Ok(())
}

/// Create and publish new script to the API
async fn publish_new_script(
    client: &Client,
    script_path: &Path,
    script_name: &str,
    project_id: &str,
) -> Result<()> {
    let mut script_config = ScriptConfig::from_path(script_path).map_err(|e| Error::Script(e))?;

    let script = client
        .create_script()
        .project(project_id)
        .body(CreateScriptDto::builder().public_identifier(script_name))
        .send()
        .await
        .map_err(progenitor_error)?;

    script_config.id = Some(script.id.clone());
    script_config
        .write_config(&script_path.to_path_buf())
        .map_err(|e| Error::Io(e))?;

    client
        .create_revision()
        .id(&script.id)
        .body(
            CreateRevisionDto::builder()
                .bundle(script_config.to_bundle().map_err(|e| Error::Script(e))?)
                .script_config(script_config),
        )
        .send()
        .await
        .map_err(|e| Error::Api(format!("Failed to upload revision: {}", e)))?;

    println!(
        "🚀 Script published to {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );

    Ok(())
}
