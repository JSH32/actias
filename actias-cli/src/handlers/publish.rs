use colored::*;
use inquire::{Confirm, Text};
use std::path::Path;

use crate::{
    client::{
        Client,
        types::{CreateRevisionDto, CreateScriptDto},
    },
    errors::{Error, Result, progenitor_error},
    script::ScriptConfig,
    util::get_dir,
};

/// Handle publish command
pub async fn handle(client: &Client, script_dir: &str) -> Result<()> {
    let script_path = get_dir(script_dir, false, false).map_err(|e| Error::Io(e))?;

    let mut script_config = ScriptConfig::from_path(&script_path).map_err(|e| Error::Script(e))?;

    // Get or create script
    let script = match &script_config.id {
        Some(v) => client
            .get_script()
            .id(v)
            .send()
            .await
            .map_err(progenitor_error)?
            .into_inner(),
        None => create_new_script(client, &mut script_config, &script_path).await?,
    };

    // Create revision
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

/// Create a new script when ID is not present
async fn create_new_script(
    client: &Client,
    script_config: &mut ScriptConfig,
    script_path: &Path,
) -> Result<crate::client::types::ScriptDto> {
    if !Confirm::new("Script doesn't have an ID, would you like to create a new one?")
        .with_default(false)
        .prompt()
        .map_err(|e| Error::Command(e.to_string()))?
    {
        return Err(Error::Command(
            "Can't publish script without an ID".to_owned(),
        ));
    }

    let script_name = Text::new("What would you like the public identifier to be?")
        .prompt()
        .map_err(|e| Error::Command(e.to_string()))?;

    let project_select = Text::new("What project ID should this be under?")
        .prompt()
        .map_err(|e| Error::Command(e.to_string()))?;

    let script = client
        .create_script()
        .project(&project_select)
        .body(CreateScriptDto::builder().public_identifier(script_name))
        .send()
        .await
        .map_err(progenitor_error)?;

    println!(
        "📜 Script has been created {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black()
    );

    script_config.id = Some(script.id.clone());

    // Write the new ID to the config.
    script_config
        .write_config(&script_path.to_path_buf())
        .map_err(|e| Error::Io(e))?;

    Ok(script.into_inner())
}
