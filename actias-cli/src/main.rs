mod client;
mod commands;
mod errors;
mod handlers;
mod router;
mod script;
mod settings;
mod util;

use clap::Parser;
use commands::{Cli, Commands};
use dirs::config_dir;
use errors::{Error, print_error};
use reqwest::header;
use router::Router;
use settings::Settings;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        print_error(&err);
    }
}

async fn run() -> errors::Result<()> {
    let cli = Cli::parse();

    // Parsing settings should trigger a re-auth.
    let relog = if let Commands::Login = cli.command {
        std::fs::remove_file(
            config_dir()
                .unwrap()
                .join("actias-cli")
                .join("settings.json"),
        )
        .map_err(|e| Error::Io(format!("Failed to remove settings file: {}", e)))?;
        true
    } else {
        false
    };

    // Set up client
    let settings = Settings::new(relog)
        .await
        .map_err(|e| Error::Authentication(e))?;

    let auth_header = format!("Bearer {}", settings.token);

    let mut headers = header::HeaderMap::new();
    headers.insert(
        "Authorization",
        header::HeaderValue::from_str(&auth_header)
            .map_err(|e| Error::Authentication(format!("Invalid token format: {}", e)))?,
    );

    let req_client = reqwest::ClientBuilder::new()
        .default_headers(headers)
        .build()
        .map_err(|e| Error::Io(format!("Failed to build HTTP client: {}", e)))?;

    let client = client::Client::new_with_client(&settings.api_url, req_client);

    // Route command to appropriate handler
    let router = Router::new(client);
    router.route(cli.command).await?;

    Ok(())
}
