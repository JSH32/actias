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
    client::types::{BundleDto, FileDto},
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
    /// List of all files which will be included in their bundle.
    /// All paths are relative to the project file.
    pub includes: Vec<String>,
    /// Patterns to ignore. This will be cross referenced with `includes`.
    pub ignore: Vec<String>,
}

impl ScriptConfig {
    /// Parse project directory to a proper [`ScriptConfig`].
    pub fn from_path(project_path: &Path) -> Result<Self, String> {
        let mut config_path = project_path.to_path_buf();
        config_path.push("script.json");

        if !config_path.exists() {
            return Err(format!(
                "{} is missing from the provided directory!",
                "script.json".yellow()
            ));
        }

        let reader: BufReader<File> = BufReader::new(File::open(config_path).unwrap());
        let mut config: ScriptConfig = serde_json::from_reader(reader).map_err(|e| {
            format!(
                "Problem parsing {}, error: {}",
                "script.json".yellow(),
                e.to_string()
            )
        })?;

        util::copy_definitions(&project_path.to_path_buf())?;

        config.project_path = Some(project_path.to_path_buf());
        Ok(config)
    }

    /// Get a bundle object from the project configuration.
    pub fn to_bundle(&self) -> Result<BundleDto, String> {
        let file_paths = self.glob_includes()?;

        let mut files = vec![];
        for file in file_paths {
            files.push(FileDto {
                content: base64::engine::general_purpose::STANDARD_NO_PAD
                    .encode(fs::read(file.clone()).unwrap()),
                file_name: file.file_name().unwrap().to_str().unwrap().to_owned(),
                file_path: file
                    .strip_prefix(self.project_path.clone().unwrap().as_path())
                    .map_err(|e| e.to_string())?
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

    pub fn write_config(&self, script_path: &PathBuf) -> Result<(), String> {
        // Write the new ID to the config.
        let mut config = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open({
                let mut script_config = script_path.clone();
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
            for entry in glob.walk(self.project_path.clone().unwrap()) {
                if let Ok(entry) = entry {
                    ignores.push(entry.into_path());
                }
            }
        }

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

        Ok(includes
            .into_iter()
            .filter(|item| !ignores.contains(item))
            .collect())
    }
}
