mod client;
mod script;
mod settings;
mod util;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use client::{
    types::{CreateScriptDto, ScriptDto},
    Client,
};

use dirs::config_dir;
use include_dir::{include_dir, Dir};
use inquire::{Confirm, Text};

use colored::*;
use prettytable::{row, Table};
use reqwest::header;
use settings::Settings;
use util::write_revision;

use crate::{
    client::types::CreateRevisionDto,
    script::ScriptConfig,
    util::{copy_definitions, get_dir, progenitor_error},
};

static PROJ_TEMPLATE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template/templates");

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
    /// 🔑 Login to an Actias account.
    Login,
    /// 📜 Initialize a new sample project
    Init {
        /// Folder name of the new project
        name: String,
        /// Template name
        template: Option<String>,
        /// Id of the project to create the script under.
        project_id: Option<String>,
    },
    /// 🚀 Publish a new revision of the project
    Publish {
        /// Directory of project to publish
        directory: String,
    },
    /// 📁 List projects
    Projects { page: Option<i64> },
    /// 📜 Manage a project
    Project {
        /// Project to manage.
        id: String,
        #[clap(subcommand)]
        sub: ProjectOperations,
    },
    /// 📑 List scripts
    Scripts { project: String, page: Option<i64> },
    /// 📜 Manage a script
    Script {
        /// Script to manage.
        id: String,
        #[clap(subcommand)]
        sub: ScriptOperations,
    },
    /// Check a project config and generate definitions.
    Check {
        /// Directory of project
        directory: String,
    },
}

#[derive(Parser, Debug)]
enum ProjectOperations {
    /// 🚮 Delete a project and all resources.
    Delete,
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
    /// Clone the most recent revision to filesystem.
    Clone { path: Option<String> },
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
    Clone {
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

    // Parsing settings should trigger a re-auth.
    let relog = if let Commands::Login = cli.command {
        let _ = std::fs::remove_file(
            config_dir()
                .unwrap()
                .join("actias-cli")
                .join("settings.json"),
        );
        true
    } else {
        false
    };

    let (api_url, auth_header) = match Settings::new(relog).await {
        Ok(v) => (v.api_url, format!("Bearer {}", v.token)),
        Err(e) => {
            println!("❌ Error while authenticating, {}", e.to_string());
            return;
        }
    };

    let mut headers = header::HeaderMap::new();
    headers.insert(
        "Authorization",
        header::HeaderValue::from_str(&auth_header).unwrap(),
    );

    let req_client = reqwest::ClientBuilder::new()
        .default_headers(headers)
        .build()
        .unwrap();

    let client = Client::new_with_client(&api_url, req_client);

    match cli.command {
        Commands::Init {
            project_id,
            name,
            template,
        } => {
            if let Err(e) = create_script(&client, &name, project_id, template).await {
                println!("❌ Error while initializing project, {}", e.to_string())
            }
        }
        Commands::Publish { directory } => {
            if let Err(e) = publish_script(&client, &directory).await {
                println!("❌ Error while publishing project, {}", e.to_string())
            }
        }
        Commands::Scripts { project, page } => {
            if let Err(e) = list_scripts(&client, &project, page.unwrap_or(1) as f64).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
        Commands::Projects { page } => {
            if let Err(e) = list_projects(&client, page.unwrap_or(1) as f64).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
        Commands::Project { id, sub } => {
            if let Err(e) = project_manage_command(&client, &id, &sub).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
        Commands::Script { id, sub } => {
            if let Err(e) = script_manage_command(&client, &id, &sub).await {
                println!("❌ Error, {}", e.to_string())
            }
        }
        Commands::Check { directory } => match ScriptConfig::from_path(Path::new(&directory)) {
            Ok(_) => println!("{}", "📜 Project validated!".green()),
            Err(e) => println!("❌ Error, {}", e.to_string()),
        },
        // This is pretty much only needed because of Commands::Login
        _ => {}
    };
}

async fn project_manage_command(
    client: &Client,
    id: &str,
    command: &ProjectOperations,
) -> Result<(), String> {
    let project = client
        .get_project()
        .project(id)
        .send()
        .await
        .map_err(progenitor_error)?;

    match command {
        ProjectOperations::Delete => {
            println!(
                "🚮 Deleted project {} {}",
                project.name.purple(),
                format!("({})", project.id).bright_black()
            )
        }
    }

    Ok(())
}

/// TODO: List perimssions that the logged in user can do.
async fn list_projects(client: &Client, page: f64) -> Result<(), String> {
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

async fn list_scripts(client: &Client, project: &str, page: f64) -> Result<(), String> {
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

async fn script_manage_command(
    client: &Client,
    script_id: &str,
    command: &ScriptOperations,
) -> Result<(), String> {
    let path = Path::new(script_id);
    let id = match ScriptConfig::from_path(&path) {
        Ok(v) => v.id.unwrap_or(script_id.to_string()),
        Err(_) => script_id.to_string(),
    };

    match command {
        ScriptOperations::Delete => {
            let script = client
                .get_script()
                .id(id.clone())
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            client
                .delete_script()
                .id(id)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            println!(
                "🚮 Deleted script {} {}",
                script.public_identifier.purple(),
                format!("({})", script.id).bright_black()
            )
        }
        ScriptOperations::Clone { path } => {
            let script = client
                .get_script()
                .id(id)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            match &script.current_revision_id {
                Some(v) => {
                    clone_revision(client, &script, &v, path.clone()).await?;
                }
                None => {
                    println!("Script does not have a current revision");
                    if Confirm::new("Do you want to create one?")
                        .with_default(true)
                        .with_help_message("This will create a sample revision")
                        .prompt()
                        .unwrap()
                    {
                        let script_path = get_dir(&script.public_identifier, true, true)?;

                        let template_names: Vec<String> = PROJ_TEMPLATE_DIR
                            .dirs()
                            .map(|f| f.path().file_name().unwrap().to_string_lossy().into_owned())
                            .collect();

                        let template = inquire::Select::new(
                            "What template would you like to use?",
                            template_names,
                        )
                        .prompt()
                        .map_err(|e| e.to_string())?;

                        // This is a workaround that creates the template dir and then copies it out and deletes the temp dir.
                        {
                            let mut temp_path = script_path.clone();
                            temp_path.push(&template);

                            std::fs::create_dir_all(temp_path.clone()).unwrap();

                            PROJ_TEMPLATE_DIR
                                .get_dir(&template)
                                .unwrap()
                                .extract(&script_path)
                                .map_err(|e| e.to_string())?;

                            util::copy_dir_all(&temp_path, &script_path).unwrap();
                            std::fs::remove_dir_all(temp_path).unwrap();
                        }

                        let mut script_config = ScriptConfig::from_path(&script_path).unwrap();
                        script_config.id = Some(script.id.clone());
                        script_config.write_config(&script_path)?;

                        copy_definitions(&script_path)?;

                        publish_script(client, &script_path.to_str().unwrap()).await?;
                    }
                }
            };
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
        .get_script()
        .id(script_id)
        .send()
        .await
        .map_err(progenitor_error)?;

    match command {
        RevisionCommands::Delete { revision_id } => {
            let result = client
                .delete_revision()
                .id(revision_id)
                .send()
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
                .revision_list()
                .id(script_id)
                .page(page.unwrap_or(1) as f64)
                .send()
                .await
                .map_err(progenitor_error)?;

            let mut table = Table::new();
            table.add_row(row!["ID", "Created",]);

            println!(
                "🔍 Displaying script page {} of {}",
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
        }
        RevisionCommands::Set { revision_id } => {
            let response = client
                .set_revision()
                .id(script_id)
                .revision_id(revision_id)
                .send()
                .await
                .map_err(progenitor_error)?;

            println!(
                "✅ Set {}'s {} revision to {}",
                script.public_identifier.purple(),
                format!("({})", response.script_id).bright_black(),
                response.revision_id.clone().unwrap().yellow()
            )
        }
        RevisionCommands::Clone { revision_id, path } => {
            let revision_id = revision_id
                .clone()
                .unwrap_or(script.clone().current_revision_id.ok_or("".to_string())?);

            clone_revision(client, &script, &revision_id, path.clone()).await?;
        }
    }

    Ok(())
}

async fn clone_revision(
    client: &Client,
    script: &ScriptDto,
    revision_id: &str,
    path: Option<String>,
) -> Result<(), String> {
    let revision = client
        .get_revision()
        .id(revision_id)
        .with_bundle(true)
        .send()
        .await
        .map_err(progenitor_error)?;

    let mut script_path = PathBuf::from(std::env::current_dir().unwrap());
    script_path.push(script.public_identifier.clone());

    let path = path
        .clone()
        .map(|p| PathBuf::from(p))
        .unwrap_or(script_path);

    write_revision(path.clone(), revision.clone())?;
    copy_definitions(&path)?;

    println!(
        "📥 Cloned revision {} for {} {}",
        format!("({})", revision.id).bright_black(),
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );

    Ok(())
}

async fn publish_script(client: &Client, script_dir: &str) -> Result<(), String> {
    let script_path = get_dir(script_dir, false, false)?;

    let mut script_config = ScriptConfig::from_path(&script_path)?;

    let script = match &script_config.id {
        Some(v) => client
            .get_script()
            .id(v)
            .send()
            .await
            .map_err(progenitor_error)?,
        None => {
            if !Confirm::new("Script doesn't have an ID, would you like to create a new one?")
                .with_default(false)
                .prompt()
                .map_err(|e| e.to_string())?
            {
                return Err("can't publish script without an ID".to_owned());
            }

            let script_name = Text::new("What would you like the public identifier to be?")
                .prompt()
                .map_err(|e| e.to_string())?;

            let project_select = Text::new("What project ID should this be under?")
                .prompt()
                .map_err(|e| e.to_string())?;

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
            script_config.write_config(&script_path)?;

            script
        }
    };

    client
        .create_revision()
        .id(&script.id)
        .body(
            CreateRevisionDto::builder()
                .bundle(script_config.to_bundle()?)
                .script_config(script_config),
        )
        .send()
        .await
        .map_err(|e| format!("failed to upload revision: {}", e.to_string()))?;

    println!(
        "🚀 Script published to {} {}",
        script.public_identifier.purple(),
        format!("({})", script.id).bright_black(),
    );

    Ok(())
}

async fn create_script(
    client: &Client,
    script_name: &str,
    project_id: Option<String>,
    template: Option<String>,
) -> Result<(), String> {
    // Ensure access
    if let Some(project_id) = &project_id {
        let acl: progenitor::progenitor_client::ResponseValue<client::types::AclListDto> = client
            .get_acl_me()
            .project(project_id)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !acl.permissions.contains_key("SCRIPT_WRITE") {
            return Err("No permission to write scripts.".to_string());
        }
    }

    let names: Vec<String> = PROJ_TEMPLATE_DIR
        .dirs()
        .map(|f| f.path().file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    let (template_name, template) = if let Some(template) = template {
        if !names.contains(&template) {
            return Err(format!(
                "Template {} not found, available templates: {:#?}",
                template, names
            ));
        } else {
            (
                template.clone(),
                PROJ_TEMPLATE_DIR.get_dir(&template).unwrap(),
            )
        }
    } else {
        let template = inquire::Select::new("What template would you like to use?", names)
            .prompt()
            .map_err(|e| e.to_string())?;

        (
            template.clone(),
            PROJ_TEMPLATE_DIR.get_dir(&template).unwrap(),
        )
    };

    let script_path = get_dir(script_name, true, true)?;

    // This is a workaround that creates the template dir and then copies it out and deletes the temp dir.
    {
        let mut temp_path = script_path.clone();
        temp_path.push(template_name);

        std::fs::create_dir_all(temp_path.clone()).unwrap();

        template.extract(&script_path).map_err(|e| e.to_string())?;

        util::copy_dir_all(&temp_path, &script_path).unwrap();
        std::fs::remove_dir_all(temp_path).unwrap();
    }

    copy_definitions(&script_path)?;

    println!(
        "📜 Script {} was created!",
        script_path.file_name().unwrap().to_str().unwrap().magenta()
    );

    // Create a script and publish a revision.
    if let Some(project_id) = project_id {
        let mut script_config = ScriptConfig::from_path(&script_path)?;

        let script = client
            .create_script()
            .project(project_id)
            .body(CreateScriptDto::builder().public_identifier(script_name))
            .send()
            .await
            .map_err(progenitor_error)?;

        script_config.id = Some(script.id.clone());
        script_config.write_config(&script_path)?;

        client
            .create_revision()
            .id(&script.id)
            .body(
                CreateRevisionDto::builder()
                    .bundle(script_config.to_bundle()?)
                    .script_config(script_config),
            )
            .send()
            .await
            .map_err(|e| format!("failed to upload revision: {}", e.to_string()))?;

        println!(
            "🚀 Script published to {} {}",
            script.public_identifier.purple(),
            format!("({})", script.id).bright_black(),
        );
    }

    Ok(())
}
