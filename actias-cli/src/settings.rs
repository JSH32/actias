use std::{fs::File, io::BufReader};

use colored::Colorize;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// API url.
    pub api_url: String,
    /// API auth token.
    pub token: String,
}

impl Settings {
    pub fn new() -> Result<Self, String> {
        let reader: BufReader<File> = BufReader::new(File::open("settings.json").unwrap());
        let settings: Settings = serde_json::from_reader(reader).map_err(|e| {
            format!(
                "Problem parsing {}, error: {}",
                "settings.json".yellow(),
                e.to_string()
            )
        })?;

        Ok(settings)
    }
}
