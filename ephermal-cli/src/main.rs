mod client;

use std::{
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    vec,
};

use base64::Engine;

use clap::{Parser, Subcommand};
use client::{
    types::{BundleDto, CreateScriptDto, FileDto},
    Client,
};
use include_dir::{include_dir, Dir};
use inquire::{Confirm, Text};
use serde::{Deserialize, Serialize};

use colored::*;
use wax::Glob;

use crate::client::types::CreateRevisionDto;

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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectConfig {
    #[serde(skip)]
    project_path: Option<PathBuf>,

    /// ID of the project, this will be null at first.
    /// On the first upload this will be set.
    id: Option<String>,
    /// First file which will be executed in the bundle.
    entry_point: String,
    /// List of all files which will be included in their bundle.
    /// All paths are relative to the project file.
    includes: Vec<String>,
}

impl ProjectConfig {
    /// Parse project directory to a proper [`ProjectConfig`].
    fn from_path(project_path: &Path) -> Result<Self, String> {
        let mut config_path = project_path.to_path_buf();
        config_path.push("project.json");

        if !config_path.exists() {
            return Err(format!(
                "{} is missing from the provided directory!",
                "project.json".yellow()
            ));
        }

        let reader = BufReader::new(File::open(config_path).unwrap());
        let mut config: ProjectConfig = serde_json::from_reader(reader).map_err(|e| {
            format!(
                "Problem parsing {}, error: {}",
                "project.json".yellow(),
                e.to_string()
            )
        })?;

        config.project_path = Some(project_path.to_path_buf());
        Ok(config)
    }

    /// Get a bundle object from the project configuration.
    fn to_bundle(&self) -> Result<BundleDto, String> {
        let file_paths = self.glob_includes()?;

        let mut files = vec![];
        for file in file_paths {
            files.push(FileDto {
                content: base64::engine::general_purpose::STANDARD_NO_PAD
                    .encode(fs::read_to_string(file.clone()).unwrap()),
                file_name: file.file_name().unwrap().to_str().unwrap().to_owned(),
                file_path: pathdiff::diff_paths(self.project_path.clone().unwrap(), file)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
                revision_id: None,
            })
        }

        Ok(BundleDto {
            entry_point: self.entry_point.clone(),
            files: files,
        })
    }

    fn glob_includes(&self) -> Result<Vec<PathBuf>, String> {
        let mut includes = vec![];
        for include in &self.includes {
            let glob = Glob::new(include).map_err(|_| "Failed to read glob pattern".to_owned())?;
            for entry in glob.walk(self.project_path.clone().unwrap()) {
                if let Ok(entry) = entry {
                    let path = entry.into_path();
                    if path.is_file() && path.file_name().unwrap() != "project.json" {
                        includes.push(path)
                    }
                }
            }
        }

        Ok(includes)
    }
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

fn progenitor_error(error: progenitor::progenitor_client::Error) -> String {
    match error {
        progenitor::progenitor_client::Error::UnexpectedResponse(e) => {
            #[derive(Deserialize)]
            struct ErrorResponse {
                message: String,
            }

            let json: ErrorResponse = futures::executor::block_on(e.json()).unwrap();
            json.message
        }
        // TODO: Document errors so we don't have to do the above hack.
        _ => error.to_string(),
    }
}

fn get_dir(relative_path: &str, should_empty: bool) -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    dir.push(relative_path);

    if should_empty && dir.exists() {
        if dir.is_dir() && dir.read_dir().unwrap().next().is_some() {
            return Err(format!(
                "specified directory {} is not empty",
                dir.to_str().unwrap().yellow()
            ));
        } else {
            // Is a file
            return Err(format!(
                "specified directory {} is a file",
                dir.to_str().unwrap().yellow()
            ));
        }
    }

    Ok(dir)
}

async fn publish_project(client: &Client, project_dir: &str) -> Result<(), String> {
    let project_path = get_dir(project_dir, false)?;

    let mut project_config = ProjectConfig::from_path(&project_path)?;

    let script = match project_config.id {
        Some(v) => client
            .scripts_controller_get_script(Some(&v), None, None)
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

            script
        }
    };

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
    let project_path = get_dir(project_name, true)?;

    fs::create_dir(&project_path)
        .map_err(|e| format!("couldn't create directory: {}", e.to_string()))?;

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
