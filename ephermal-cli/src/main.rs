mod client;
mod project;
mod util;

use std::io::Write;

use clap::{Parser, Subcommand};
use client::{types::CreateScriptDto, Client};
use include_dir::{include_dir, Dir};
use inquire::{Confirm, Text};

use colored::*;

use crate::{
    client::types::CreateRevisionDto,
    project::ProjectConfig,
    util::{get_dir, progenitor_error},
};

static TEMPLATE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template");

/// Ephermal CLI for interacting with the ephermal API.
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
    };
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
