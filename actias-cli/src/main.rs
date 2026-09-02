//! The `actias` cli: publishing a project, following its logs, running
//! a live session, and querying a project's resources from a terminal.

mod analyze;
mod capabilities;
mod client;
mod commands;
mod errors;
mod gateway;
mod handlers;
mod lsp;
mod router;
mod script;
mod service;
mod settings;
mod testing;
mod ui;
mod util;

use clap::Parser;
use commands::{Cli, Commands};
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
    match cli.command {
        Commands::Test { directory } => {
            return handlers::test::handle(&directory.unwrap_or_else(|| ".".to_owned())).await;
        }
        Commands::Check { ref directory } => {
            return handlers::check::handle(directory);
        }
        // An editor starting its language server must never be asked to
        // log in, so this belongs with the other local commands.
        Commands::Lsp => {
            return lsp::serve().map_err(Error::Script);
        }
        Commands::Sql {
            ref database,
            ref sub,
        } => {
            return handlers::sql::handle(database, sub);
        }
        _ => {}
    }

    // Parsing settings should trigger a re-auth.
    let relog = if let Commands::Login = cli.command {
        let setting_path = settings::settings_path().map_err(Error::Config)?;

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
