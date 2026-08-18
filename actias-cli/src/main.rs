mod capabilities;
mod client;
mod commands;
mod errors;
mod gateway;
mod handlers;
mod router;
mod script;
mod settings;
mod testing;
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
        // Scripts and CI branch on the exit code, so a printed error must
        // also be a nonzero exit.
        std::process::exit(1);
    }
}

async fn run() -> errors::Result<()> {
    let cli = Cli::parse();

    // Fully local commands never touch the api, so they must not demand a
    // login; ci runs them on machines with no session at all.
    if let Commands::Test { directory } = cli.command {
        return handlers::test::handle(&directory.unwrap_or_else(|| ".".to_owned())).await;
    }

    // Parsing settings should trigger a re-auth.
    let relog = if let Commands::Login = cli.command {
        let setting_path = config_dir()
            .unwrap()
            .join("actias-cli")
            .join("settings.json");

        if std::fs::exists(&setting_path)? {
            std::fs::remove_file(setting_path)
                .map_err(|e| Error::Io(format!("Failed to remove settings file: {}", e)))?;
        }
        true
    } else {
        false
    };

    // Set up client
    let settings = Settings::new(relog).await.map_err(Error::Authentication)?;

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
    let router = Router::new(client, settings);
    router.route(cli.command).await?;

    Ok(())
}
