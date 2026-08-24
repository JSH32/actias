use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use base64::Engine;
use colored::Colorize;
use serde::Deserialize;

use crate::{client::types::RevisionFullDto, script::ScriptConfig};

/// Convert an API error to a string which can be used to log.
pub fn progenitor_error(error: progenitor::progenitor_client::Error) -> String {
    #[derive(Deserialize)]
    struct ErrorResponse {
        message: String,
    }

    match error {
        progenitor::progenitor_client::Error::UnexpectedResponse(e) => {
            let json: ErrorResponse = futures::executor::block_on(e.json()).unwrap();
            json.message.to_string()
        }
        progenitor::progenitor_client::Error::CommunicationError(v) => {
            format!("Communication error encountered with the server: {}", v)
        }
        progenitor::progenitor_client::Error::ErrorResponse(v) => {
            let json: ErrorResponse = serde_json::from_value({
                let _: () = v.into_inner();
                ().into()
            })
            .unwrap();
            json.message.to_string()
        }
        progenitor::progenitor_client::Error::InvalidResponsePayload(v) => {
            format!("Response payload was invalid: {}", v)
        }
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
        fs::create_dir(&dir).map_err(|e| format!("couldn't create directory: {}", e))?;
    } else {
        return Err(format!(
            "directory {} doesn't exist",
            dir.to_str().unwrap().yellow()
        ));
    }

    Ok(dir)
}

/// Writes the project's editor tooling: the Luau declarations, a
/// luau-lsp settings file (only when none exists, so user settings
/// survive), and a .luaurc.
pub fn copy_definitions(proj_path: &Path) -> Result<(), String> {
    let def_path = proj_path.join("definitions");
    let _ = std::fs::remove_dir_all(&def_path);
    std::fs::create_dir_all(&def_path).map_err(|e| e.to_string())?;
    for (name, content) in crate::analyze::DEFINITION_FILES {
        std::fs::write(def_path.join(name), content).map_err(|e| e.to_string())?;
    }

    let vscode = proj_path.join(".vscode");
    let settings = vscode.join("settings.json");
    if !settings.exists() {
        std::fs::create_dir_all(&vscode).map_err(|e| e.to_string())?;
        std::fs::write(
            &settings,
            "{\n  \"luau-lsp.types.definitionFiles\": [\"definitions/core.d.luau\", \"definitions/objects.d.luau\", \"definitions/work.d.luau\", \"definitions/http.d.luau\"],\n  \"luau-lsp.require.mode\": \"relativeToWorkspaceRoot\"\n}\n",
        )
        .map_err(|e| e.to_string())?;
    }

    let luaurc = proj_path.join(".luaurc");
    if !luaurc.exists() {
        std::fs::write(&luaurc, "{\n  \"languageMode\": \"nonstrict\"\n}\n")
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Write a revision to a provided path.
pub fn write_revision(path: PathBuf, revision: RevisionFullDto) -> Result<(), String> {
    check_clone_dir(&revision.script_id, &path)?;

    if path.exists() {
        // Delete path to recreate it.
        std::fs::remove_dir_all(path.clone()).map_err(|e| e.to_string())?;
    }

    // Create copy path.
    std::fs::create_dir_all(path.clone()).map_err(|e| e.to_string())?;

    // Copy the project metadata JSON.
    let project_file = serde_json::to_string_pretty(&revision.script_config).unwrap();
    let mut project_config = path.clone();
    project_config.push("script.json");
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
fn check_clone_dir(script_id: &str, path: &Path) -> Result<(), String> {
    // Things exist here
    if path.exists() && path.read_dir().unwrap().next().is_some() {
        let config =
            ScriptConfig::from_path(path).map_err(|_| "Directory not empty and not a project")?;

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
    // Truncate, otherwise overwriting with shorter content leaves the tail of
    // the previous file behind.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();
    f.write_all(&content).unwrap();
    f.flush().unwrap();
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}
