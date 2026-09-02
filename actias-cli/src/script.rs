//! A project on disk: its `script.json`, the build command it may run,
//! and the bundle its file globs select.

use std::{
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
};

use base64::Engine;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use wax::Glob;

use crate::{
    client::types::{BundleDto, FileDto, FileDtoKind, ScriptConfigDto},
    util,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScriptConfig {
    #[serde(skip)]
    pub project_path: Option<PathBuf>,

    /// ID of the project, this will be null at first.
    /// On the first upload this will be set.
    pub id: Option<String>,
    /// First file which will be executed in the bundle.
    pub entry_point: String,
    /// Glob patterns selecting the files the bundle carries.
    /// All paths are relative to the project file.
    pub includes: Vec<String>,
    /// Patterns to ignore. This will be cross referenced with `includes`.
    pub ignore: Vec<String>,
    /// Optional shell command run in the project directory before
    /// bundling on publish: the seam for a frontend build (vite,
    /// esbuild, anything) without the cli learning any tool. Whatever
    /// it writes ships only if `includes` selects it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl ScriptConfig {
    /// Runs the declared build command, if any, inheriting stdio so
    /// its output lands in the publisher's terminal; the bundle is
    /// cut only from what it leaves behind.
    ///
    /// # Errors
    /// Returns text when the project path is unset, when the build command
    /// cannot start, or when it exits nonzero.
    pub fn run_build(&self) -> Result<(), String> {
        let Some(command) = &self.build else {
            return Ok(());
        };
        let directory = self
            .project_path
            .as_ref()
            .ok_or("the project path is unset")?;
        crate::ui::step("Building", command);
        #[cfg(windows)]
        let mut shell = {
            let mut shell = std::process::Command::new("cmd");
            shell.arg("/C");
            shell
        };
        #[cfg(not(windows))]
        let mut shell = {
            let mut shell = std::process::Command::new("sh");
            shell.arg("-c");
            shell
        };
        let status = shell
            .arg(command)
            .current_dir(directory)
            .status()
            .map_err(|e| format!("the build command could not start: {e}"))?;
        if !status.success() {
            return Err(format!("the build command exited with {status}"));
        }
        Ok(())
    }
}

impl From<ScriptConfig> for ScriptConfigDto {
    fn from(val: ScriptConfig) -> Self {
        ScriptConfigDto {
            id: val.id.unwrap(),
            entry_point: val.entry_point,
            includes: val.includes,
            ignore: val.ignore,
            // Filled by the publish path from the declaration pass; other
            // callers (live sessions) carry no contract.
            capabilities: None,
        }
    }
}

impl ScriptConfig {
    /// Reads a project directory's `script.json` into a [`ScriptConfig`].
    ///
    /// # Errors
    /// Returns text when the directory holds no `script.json`, or when it
    /// does not parse.
    pub fn from_path(project_path: &Path) -> Result<Self, String> {
        let mut config_path = project_path.to_path_buf();
        config_path.push("script.json");

        if !config_path.exists() {
            return Err(format!(
                "{} is missing from the provided directory",
                "script.json".yellow()
            ));
        }

        let reader: BufReader<File> = BufReader::new(File::open(config_path).unwrap());
        let mut config: ScriptConfig = serde_json::from_reader(reader)
            .map_err(|e| format!("Problem parsing {}, error: {}", "script.json".yellow(), e))?;

        util::copy_definitions(project_path)?;

        config.project_path = Some(project_path.to_path_buf());
        Ok(config)
    }

    /// Builds the bundle this project's configuration describes.
    ///
    /// # Errors
    /// Returns text naming the file that could not be read.
    pub fn to_bundle(&self) -> Result<BundleDto, String> {
        let file_paths = self.glob_includes()?;

        let mut files = vec![];
        for file in file_paths {
            let file_path = file
                .strip_prefix(self.project_path.clone().unwrap().as_path())
                .map_err(|e| e.to_string())?
                .to_str()
                .unwrap()
                .to_string();

            let bytes = fs::read(file.clone()).map_err(|e| format!("{file_path}: {e}"))?;

            files.push(FileDto {
                content: base64::engine::general_purpose::STANDARD_NO_PAD.encode(&bytes),
                content_type: Some(content_type_for(&file).to_owned()),
                kind: Some(if file_path.ends_with(".lua") {
                    FileDtoKind::Module
                } else {
                    FileDtoKind::Asset
                }),
                file_path,
                // The same blake3 the store computes; publish negotiates with
                // it so unchanged files never resend their content.
                hash: Some(blake3::hash(&bytes).to_hex().to_string()),
                size: None,
            })
        }

        Ok(BundleDto {
            entry_point: self.entry_point.clone(),
            files,
        })
    }

    pub fn write_config(&self, script_path: &Path) -> Result<(), String> {
        // Write the new ID to the config.
        let mut config = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open({
                let mut script_config = script_path.to_path_buf();
                script_config.push("script.json");
                script_config
            })
            .unwrap();

        config
            .write_all(serde_json::to_string_pretty(&self).unwrap().as_bytes())
            .unwrap();

        config.flush().unwrap();

        Ok(())
    }

    fn glob_includes(&self) -> Result<Vec<PathBuf>, String> {
        let mut ignores = vec![];
        for ignore in &self.ignore {
            let glob = Glob::new(ignore).map_err(|_| "Failed to read glob pattern".to_owned())?;
            for entry in glob.walk(self.project_path.clone().unwrap()).flatten() {
                ignores.push(entry.into_path());
            }
        }

        let mut includes = vec![];
        for include in &self.includes {
            let glob = Glob::new(include).map_err(|_| "Failed to read glob pattern".to_owned())?;
            for entry in glob.walk(self.project_path.clone().unwrap()).flatten() {
                let path = entry.into_path();
                if path.is_file() && path.file_name().unwrap() != "project.json" {
                    includes.push(path)
                }
            }
        }

        Ok(includes
            .into_iter()
            .filter(|item| !ignores.contains(item))
            .collect())
    }
}

/// Mime type for a bundle file by extension, which the platform serves
/// assets with; unknown extensions fall back to octet-stream.
fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "lua" => "text/x-lua",
        _ => "application/octet-stream",
    }
}
