//! Manage a project's secrets. Values go up once and never come back; the
//! api stores them encrypted and only the worker decrypts them.

use colored::Colorize;
use inquire::Password;

use crate::{
    client::{Client, types::SetSecretDto},
    commands::SecretOperations,
    errors::{Error, Result, progenitor_error},
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

            println!("🔐 Secret {} set.", name.purple());
        }
        SecretOperations::List => {
            let names = client
                .list_secrets()
                .project(project)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            if names.is_empty() {
                println!("No secrets set.");
            }
            for name in names {
                println!("🔐 {}", name.purple());
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

            println!("🚮 Secret {} deleted.", name.purple());
        }
    }

    Ok(())
}
