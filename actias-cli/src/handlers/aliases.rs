//! Environment aliases: named pointers (staging, prod) from a script to a
//! revision. Rollback is `alias set` back at the previous revision.

use std::path::Path;

use colored::*;

use crate::{
    client::{Client, types::SetAliasDto},
    commands::AliasOperations,
    errors::{Result, progenitor_error},
    script::ScriptConfig,
    ui,
};

/// Handles `actias alias <script> <op>`; `script` may be a script id or a
/// project directory whose config carries one.
///
/// # Errors
/// Returns the api's message.
pub async fn handle(client: &Client, script: &str, operation: &AliasOperations) -> Result<()> {
    let id = match ScriptConfig::from_path(Path::new(script)) {
        Ok(config) => config.id.unwrap_or_else(|| script.to_owned()),
        Err(_) => script.to_owned(),
    };

    match operation {
        AliasOperations::Set { name, revision_id } => set(client, &id, name, revision_id).await,
        AliasOperations::List => list(client, &id).await,
    }
}

async fn set(client: &Client, script_id: &str, name: &str, revision_id: &str) -> Result<()> {
    let alias = client
        .set_alias()
        .id(script_id)
        .body(SetAliasDto::builder().name(name).revision_id(revision_id))
        .send()
        .await
        .map_err(progenitor_error)?;

    // The path form works on any worker; a subdomain deployment also
    // serves it at <ident>--<alias>.<base>.
    let script = client
        .get_script()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?;
    ui::done(
        "Aliased",
        format!("{} -> revision {}", alias.name, alias.revision_id),
    );
    ui::detail(format!("/_alias/{}/{}", script.public_identifier, alias.name).bright_black());

    Ok(())
}

async fn list(client: &Client, script_id: &str) -> Result<()> {
    let aliases = client
        .list_aliases()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    if aliases.is_empty() {
        println!("No aliases; create one with {}", "alias set".purple());
        return Ok(());
    }

    for alias in aliases {
        ui::detail(format!("{} -> {}", alias.name, alias.revision_id));
    }

    Ok(())
}
