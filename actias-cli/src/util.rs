use std::{fs, io::Write, path::PathBuf};

use base64::Engine;
use colored::Colorize;
use include_dir::{include_dir, Dir};
use serde::Deserialize;

use crate::{client::types::RevisionFullDto, project::ProjectConfig};

/// Convert an API error to a string which can be used to log.
pub fn progenitor_error(error: progenitor::progenitor_client::Error) -> String {
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

/// Get a directory and validate it. This will create a directory of it doesn't exist.
///
/// # Arguments
/// * `relative_path` - Relative path to the current terminal.
/// * `should_empty` - Does the path have to be empty.
/// * `create_dir` - Should this create the directory if it doesn't exist? Otherwise an error will be thrown.
pub fn get_dir(
    relative_path: &str,
    should_empty: bool,
    create_dir: bool,
) -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    dir.push(relative_path);

    if dir.exists() {
        // Is a file
        if !dir.is_dir() {
            return Err(format!(
                "specified directory {} is a file",
                dir.to_str().unwrap().yellow()
            ));
        }

        // Directory contains items
        if should_empty && dir.read_dir().unwrap().next().is_some() {
            return Err(format!(
                "specified directory {} is not empty",
                dir.to_str().unwrap().yellow()
            ));
        }
    } else if create_dir {
        fs::create_dir(&dir)
            .map_err(|e| format!("couldn't create directory: {}", e.to_string()))?;
    } else {
        return Err(format!(
            "directory {} doesn't exist",
            dir.to_str().unwrap().yellow()
        ));
    }

    Ok(dir)
}

pub fn copy_definitions(proj_path: &PathBuf) -> Result<(), String> {
    static DEFINITION_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/template/definitions");

    let mut def_path = proj_path.clone();
    def_path.push("definitions");

    // Recreate definitions
    let _ = std::fs::remove_dir(def_path.clone());
    std::fs::create_dir_all(def_path.clone()).unwrap();

    DEFINITION_DIR
        .extract(&def_path)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Write a revision to a provided path.
pub fn write_revision(path: PathBuf, revision: RevisionFullDto) -> Result<(), String> {
    check_clone_dir(&revision.script_id, &path)?;

    // Create copy path.
    std::fs::create_dir_all(path.clone()).map_err(|e| e.to_string())?;

    // Copy the project metadata JSON.
    let project_file = serde_json::to_string_pretty(&revision.project_config).unwrap();
    let mut project_config = path.clone();
    project_config.push("project.json");
    overwrite_file(project_config, project_file.into_bytes());

    for file in revision.bundle.unwrap().files {
        let mut path = path.clone();
        path.push(file.file_path);

        overwrite_file(
            path,
            base64::engine::general_purpose::STANDARD
                .decode(file.content)
                .unwrap(),
        )
    }

    Ok(())
}

/// Check if revision can be cloned to this directory.
fn check_clone_dir(script_id: &str, path: &PathBuf) -> Result<(), String> {
    // Things exist here
    if path.exists() && path.read_dir().unwrap().next().is_some() {
        let config = ProjectConfig::from_path(path.as_path())
            .map_err(|_| "Directory not empty and not a project")?;

        if config.id.unwrap_or("".to_string()) != script_id {
            return Err(
                "Script ID in that directory doesn't match, cannot safely clone".to_string(),
            );
        }
    }

    Ok(())
}

/// Overwrite file if it exists.
pub fn overwrite_file(path: PathBuf, content: Vec<u8>) {
    let prefix_parent = path.parent().unwrap();
    std::fs::create_dir_all(prefix_parent).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)
        .unwrap();
    f.write_all(&content).unwrap();
    f.flush().unwrap();
}
