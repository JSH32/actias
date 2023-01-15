use std::{fs, path::PathBuf};

use colored::Colorize;
use serde::Deserialize;

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
