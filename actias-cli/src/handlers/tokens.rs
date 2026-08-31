//! Manage a project's service tokens: machine credentials for this api,
//! ACL-scoped like members, hash-stored, shown exactly once at creation.

use colored::Colorize;

use crate::{
    client::{Client, types::CreateServiceTokenDto},
    commands::TokenOperations,
    errors::{Error, Result, progenitor_error},
    ui,
};

/// Handle tokens command
pub async fn handle(client: &Client, project: &str, operation: &TokenOperations) -> Result<()> {
    match operation {
        TokenOperations::Create { name, access } => {
            let access = access
                .iter()
                .map(|bit| {
                    bit.to_uppercase()
                        .parse()
                        .map_err(|_| Error::Command(format!("unknown access field '{bit}'")))
                })
                .collect::<Result<Vec<_>>>()?;

            let created = client
                .create_token()
                .project(project)
                .body(CreateServiceTokenDto::builder().name(name).access(access))
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done("Created", format!("token {}", created.name));
            ui::detail(created.token.green());
            ui::warn("shown once, store it now");
        }
        TokenOperations::List => {
            let tokens = client
                .list_tokens()
                .project(project)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();

            if tokens.is_empty() {
                println!("No service tokens.");
            }
            for token in tokens {
                let used = token
                    .last_used
                    .map(|at| format!("last used {}", at.format("%Y-%m-%d %H:%M")))
                    .unwrap_or_else(|| "never used".to_owned());
                println!(
                    "{} {} {} {}",
                    token.name.purple(),
                    token.token_prefix,
                    token.id.dimmed(),
                    used.dimmed(),
                );
            }
        }
        TokenOperations::Revoke { id } => {
            client
                .revoke_token()
                .project(project)
                .token(id)
                .send()
                .await
                .map_err(progenitor_error)?;

            ui::done("Revoked", format!("token {id}"));
        }
    }

    Ok(())
}
