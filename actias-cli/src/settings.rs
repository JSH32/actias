use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
};

use colored::Colorize;
use dirs::config_dir;
use inquire::{Password, Text};
use serde::{Deserialize, Serialize};

use crate::{
    client::{types::LoginDto, Client},
    util::progenitor_error,
};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// API url.
    pub api_url: String,
    /// API auth token.
    pub token: String,
}

impl Settings {
    pub async fn new(relog: bool) -> Result<Self, String> {
        let config_dir = config_dir().unwrap();
        let auth_dir = config_dir.join("actias-cli");
        std::fs::create_dir_all(&auth_dir).map_err(|e| e.to_string())?;

        let settings_file = auth_dir.join("settings.json");

        if !settings_file.as_path().exists() {
            if !relog {
                println!("🔑 You are not logged in!");
            }

            let server_url = Text::new("What is the server URL?")
                .prompt()
                .map_err(|e| e.to_string())?;

            let client = Client::new(&server_url);

            let username = Text::new("What is your username/email?")
                .prompt()
                .map_err(|e| e.to_string())?;

            let password = Password::new("What is your password?")
                .without_confirmation()
                .prompt()
                .map_err(|e| e.to_string())?;

            let auth = client
                .login()
                .body(
                    LoginDto::builder()
                        .auth(username)
                        .password(password)
                        .remember_me(true),
                )
                .send()
                .await
                .map_err(progenitor_error)?;

            let settings = Settings {
                api_url: server_url,
                token: auth.token.clone(),
            };

            let mut writer = BufWriter::new(File::create(settings_file).unwrap());
            serde_json::to_writer(&mut writer, &settings).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;

            println!("{}", "🔑 Logged in successfully!".green());

            return Ok(settings);
        }

        let reader: BufReader<File> = BufReader::new(File::open(settings_file).unwrap());
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
