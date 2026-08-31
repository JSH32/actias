//! Manage a project's secrets. Values go up once and never come back; the
//! secret service stores them encrypted, versioned per rotation, and only
//! worker resolution ever sees plaintext.

use colored::Colorize;
use inquire::Password;

use crate::{
    client::{Client, types::SetSecretDto},
    commands::SecretOperations,
    errors::{Error, Result, progenitor_error},
    ui,
};

/// Handle secret command
pub async fn handle(client: &Client, project: &str, operation: &SecretOperations) -> Result<()> {
    match operation {
        SecretOperations::Put { name, value } => {
            let value = match value {
                Some(value) => value.clone(),
                None => Password::new(&format!("Value for '{name}':"))
                    .without_confirmation()
                    .prompt()
                    .map_err(|e| Error::Command(e.to_string()))?,
            };

            client
                .put_secret()
                .project(project)
                .name(name)
                .body(SetSecretDto::builder().value(value))
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done("Set", format!("secret {name}"));
        }
        SecretOperations::List => {
            let secrets = client
                .list_secrets()
                .project(project)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            if secrets.is_empty() {
                println!("No secrets set.");
            }
            for secret in secrets {
                println!(
                    "{} {}",
                    secret.name.purple(),
                    format!("v{}", secret.version as i64).dimmed()
                );
            }
        }
        SecretOperations::Delete { name } => {
            client
                .delete_secret()
                .project(project)
                .name(name)
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done("Deleted", format!("secret {name}"));
        }
    }

    Ok(())
}
