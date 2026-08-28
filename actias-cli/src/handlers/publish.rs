use colored::*;
use inquire::{Confirm, Text};
use std::path::Path;

use crate::{
    capabilities,
    client::{
        Client,
        types::{
            CapabilitiesDto, CreateRevisionDto, CreateScriptDto, MissingBlobsDto, ScriptConfigDto,
        },
    },
    errors::{Error, Result, progenitor_error},
    script::ScriptConfig,
    util::get_dir,
};

/// Handle publish command
pub async fn handle(client: &Client, script_dir: &str) -> Result<()> {
    let script_path = get_dir(script_dir, false, false).map_err(Error::Io)?;

    let mut script_config = ScriptConfig::from_path(&script_path).map_err(Error::Script)?;

    // A declared build runs first, locally, before anything touches
    // the network: its output is part of the tree the bundle is cut
    // from.
    script_config.run_build().map_err(Error::Script)?;

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

    // The declaration pass derives the capability contract from the code;
    // it also catches syntax errors before anything is uploaded.
    let declared = capabilities::extract(&script_config).map_err(Error::Script)?;
    if !declared.kv.is_empty() {
        println!("📦 Declares kv: {}", declared.kv.join(", ").purple());
    }
    if !declared.secrets.is_empty() {
        println!(
            "🔐 Declares secrets: {}",
            declared.secrets.join(", ").purple()
        );
    }
    if !declared.objects.is_empty() {
        println!(
            "🧩 Declares objects: {}",
            declared.objects.join(", ").purple()
        );
    }
    if !declared.databases.is_empty() {
        println!(
            "🗄️ Declares databases: {}",
            declared.databases.join(", ").purple()
        );
    }
    if !declared.queues.is_empty() {
        println!(
            "📬 Declares queues: {}",
            declared.queues.join(", ").purple()
        );
    }
    if !declared.connections.is_empty() {
        println!(
            "\u{1f50c} Declares connections: {}",
            declared.connections.join(", ").purple()
        );
    }
    if !declared.lifecycle.is_empty() {
        println!(
            "\u{23f3} Lifecycle: {}",
            declared.lifecycle.join(", ").purple()
        );
    }

    let mut config_dto: ScriptConfigDto = script_config.clone().into();
    config_dto.capabilities = Some(CapabilitiesDto {
        kv: declared.kv,
        events: declared.events,
        secrets: declared.secrets,
        objects: declared.objects,
        databases: declared.databases,
        queues: declared.queues,
        workflows: declared.workflows,
        workflow_steps: declared.workflow_steps,
        publishes: declared.publishes,
        lifecycle: declared.lifecycle,
        connections: declared.connections,
    });

    let mut bundle = script_config.to_bundle().map_err(Error::Script)?;

    // The store already holds any blob it has seen from anyone; files whose
    // hash it knows publish as manifest-only entries with no content.
    let hashes: Vec<String> = bundle
        .files
        .iter()
        .filter_map(|file| file.hash.clone())
        .collect();

    let missing: std::collections::HashSet<String> = client
        .missing_blobs()
        .project(&script.project_id)
        .body(MissingBlobsDto::builder().hashes(hashes))
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner()
        .missing
        .into_iter()
        .collect();

    let total = bundle.files.len();
    let mut uploading = 0;
    for file in bundle.files.iter_mut() {
        match &file.hash {
            Some(hash) if !missing.contains(hash) => file.content = String::new(),
            _ => uploading += 1,
        }
    }
    println!(
        "⬆️ Uploading {} of {} files",
        uploading.to_string().purple(),
        total.to_string().purple()
    );

    // Create revision
    let revision = client
        .create_revision()
        .id(&script.id)
        .body(
            CreateRevisionDto::builder()
                .bundle(bundle)
                .script_config(config_dto),
        )
        .send()
        .await
        .map_err(|e| Error::Api(format!("Failed to upload revision: {}", e)))?;

    println!(
        "🚀 Script published to {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );
    // The path form works on any worker; a subdomain deployment also
    // serves it at <ident>--r-<revision>.<base>.
    println!(
        "📌 Revision {} {}",
        revision.id.purple(),
        format!(
            "(preview: /_rev/{}/{})",
            script.public_identifier, revision.id
        )
        .bright_black(),
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
    script_config.write_config(script_path).map_err(Error::Io)?;

    Ok(script.into_inner())
}
