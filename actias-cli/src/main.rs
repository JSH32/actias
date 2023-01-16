mod client;
mod project;
mod util;

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use client::{types::CreateScriptDto, Client};
use include_dir::{include_dir, Dir};
use inquire::{Confirm, Text};

use colored::*;
use prettytable::{row, Table};
use util::write_revision;

use crate::{
    client::types::CreateRevisionDto,
    project::ProjectConfig,
    util::{get_dir, progenitor_error},
};

static TEMPLATE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template");

/// Actias CLI for interacting with the actias API.
#[derive(Parser, Debug)]
#[command(propagate_version = true)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 📜 Initialize a new sample project
    Init {
        /// Folder name of the new project
        name: String,
    },
    /// 🚀 Publish a new revision of the project
    Publish {
        /// Directory of project to publish
        directory: String,
    },
    /// 📑 List scripts
    Scripts { page: Option<i64> },
    /// 📜 Manage a script
    Script {
        /// Script to manage.
        id: String,
        #[clap(subcommand)]
        sub: ScriptOperations,
    },
}

#[derive(Parser, Debug)]
enum ScriptOperations {
    /// 🚮 Delete a script and all revisions.
    Delete,
    /// 🖊️ Manage revisions for this script
    Revisions {
        #[clap(subcommand)]
        sub: RevisionCommands,
    },
}

#[derive(Parser, Debug)]
enum RevisionCommands {
    /// 🚮 Delete a revision, this will try to set the script's revision to the most recent
    Delete { revision_id: String },
    /// 📑 List revisions
    List { page: Option<i64> },
    /// 📦 Set script to use a specific revision.
    Set { revision_id: String },
    /// Clone a revision to filesystem.
    Get {
        /// This will get the current active revision if not provided.
        revision_id: Option<String>,
        #[clap(short, long)]
        path: Option<String>,
    },
}

#[derive(Parser, Debug)]
struct RevisionCommand {
    #[command(subcommand)]
    command: Option<RevisionCommands>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let client = Client::new("http://localhost:3006");

    match cli.command {
        Commands::Init { name } => {
            if let Err(e) = create_project(&name) {
                println!("❌ Error while initializing project, {}", e.to_string())
            }
        }
        Commands::Publish { directory } => {
            if let Err(e) = publish_project(&client, &directory).await {
                println!("❌ Error while publishing project, {}", e.to_string())
            }
        }
        Commands::Scripts { page } => {
            if let Err(e) = list_scripts(&client, page.unwrap_or(1) as f64).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
        Commands::Script { id, sub } => {
            if let Err(e) = script_manage_command(&client, &id, &sub).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
    };
}

async fn list_scripts(client: &Client, page: f64) -> Result<(), String> {
    let response = client
        .scripts_controller_list_scripts(page)
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
        response.paginated_response_dto.page.to_string().yellow(),
        response
            .paginated_response_dto
            .total_pages
            .to_string()
            .yellow()
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

async fn script_manage_command(
    client: &Client,
    script_id: &str,
    command: &ScriptOperations,
) -> Result<(), String> {
    let path = Path::new(script_id);
    let id = match ProjectConfig::from_path(&path) {
        Ok(v) => v.id.unwrap_or(script_id.to_string()),
        Err(_) => script_id.to_string(),
    };

    match command {
        ScriptOperations::Delete => {
            let script = client
                .scripts_controller_get_script(&id)
                .await
                .map_err(progenitor_error)?
                .into_inner();

            client
                .scripts_controller_delete_script(&id)
                .await
                .map_err(progenitor_error)?
                .into_inner();

            println!(
                "🚮 Deleted {} {}",
                script.public_identifier.purple(),
                format!("({})", script.id).bright_black()
            )
        }
        ScriptOperations::Revisions { sub } => revision_command(client, &id, sub).await?,
    }

    Ok(())
}

async fn revision_command(
    client: &Client,
    script_id: &str,
    command: &RevisionCommands,
) -> Result<(), String> {
    let script = client
        .scripts_controller_get_script(&script_id)
        .await
        .map_err(progenitor_error)?;

    match command {
        RevisionCommands::Delete { revision_id } => {
            let result = client
                .revisions_controller_delete_revision(&revision_id)
                .await
                .map_err(progenitor_error)?;

            println!("🚮 Deleted revision {}", revision_id.bright_black());
            println!(
                "✅ Set {}'s {} revision to {}",
                script.public_identifier.purple(),
                format!("({})", result.script_id).bright_black(),
                match &result.revision_id {
                    Some(v) => v.yellow(),
                    None => "NONE".red(),
                }
            )
        }
        RevisionCommands::List { page } => {
            let response = client
                .scripts_controller_revision_list(script_id, page.unwrap_or(1) as f64)
                .await
                .map_err(progenitor_error)?;

            let mut table = Table::new();
            table.add_row(row!["ID", "Created",]);

            println!(
                "🔍 Displaying script page {} of {}",
                response.paginated_response_dto.page.to_string().yellow(),
                response
                    .paginated_response_dto
                    .total_pages
                    .to_string()
                    .yellow()
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
        }
        RevisionCommands::Set { revision_id } => {
            let response = client
                .scripts_controller_set_revision(script_id, revision_id)
                .await
                .map_err(progenitor_error)?;

            println!(
                "✅ Set {}'s {} revision to {}",
                script.public_identifier.purple(),
                format!("({})", response.script_id).bright_black(),
                response.revision_id.clone().unwrap().yellow()
            )
        }
        RevisionCommands::Get { revision_id, path } => {
            let revision_id = revision_id
                .clone()
                .unwrap_or(script.clone().current_revision_id.ok_or("".to_string())?);

            let revision = client
                .revisions_controller_get_revision(&revision_id, true)
                .await
                .map_err(progenitor_error)?;

            let mut script_path = PathBuf::from(std::env::current_dir().unwrap());
            script_path.push(script.public_identifier.clone());

            let path = path
                .clone()
                .map(|p| PathBuf::from(p))
                .unwrap_or(script_path);

            write_revision(path, revision.clone())?;

            println!(
                "📥 Cloned revision {} for {} {}",
                format!("({})", revision.id).bright_black(),
                script.public_identifier.purple(),
                format!("({})", script.id).bright_black(),
            )
        }
    }

    Ok(())
}

async fn publish_project(client: &Client, project_dir: &str) -> Result<(), String> {
    let project_path = get_dir(project_dir, false, false)?;

    let mut project_config = ProjectConfig::from_path(&project_path)?;

    let script = match &project_config.id {
        Some(v) => client
            .scripts_controller_get_script(&v)
            .await
            .map_err(progenitor_error)?,
        None => {
            if !Confirm::new("Project doesn't have an ID, would you like to create a new project?")
                .with_default(false)
                .prompt()
                .map_err(|e| e.to_string())?
            {
                return Err("can't publish project without an ID".to_owned());
            }

            let project_name = Text::new("What would you like the public identifier to be?")
                .prompt()
                .map_err(|e| e.to_string())?;

            let script = client
                .scripts_controller_create_script(&CreateScriptDto {
                    public_identifier: project_name,
                })
                .await
                .map_err(progenitor_error)?;

            println!(
                "📜 Script has been created {} {}",
                script.public_identifier.purple(),
                format!("({})", script.id).bright_black()
            );

            project_config.id = Some(script.id.clone());

            // Write the new ID to the config.
            let mut config = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open({
                    let mut project_config = project_path.clone();
                    project_config.push("project.json");
                    project_config
                })
                .unwrap();

            config
                .write_all(
                    serde_json::to_string_pretty(&project_config)
                        .unwrap()
                        .as_bytes(),
                )
                .unwrap();

            config.flush().unwrap();

            script
        }
    };

    client
        .scripts_controller_create_revision(
            &script.id,
            &CreateRevisionDto {
                bundle: project_config.to_bundle()?,
                project_config: serde_json::from_value(
                    serde_json::to_value(project_config).unwrap(),
                )
                .unwrap(),
            },
        )
        .await
        .map_err(|e| format!("failed to upload revision: {}", e.to_string()))?;

    println!(
        "🚀 Project published to {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );

    Ok(())
}

fn create_project(project_name: &str) -> Result<(), String> {
    let project_path = get_dir(project_name, true, true)?;

    TEMPLATE_DIR
        .extract(&project_path)
        .map_err(|e| e.to_string())?;

    println!(
        "📜 Project {} was created!",
        project_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .magenta()
    );

    Ok(())
}
