//! One parsed command to its handler, holding the client and settings
//! every handler needs.

use crate::{
    client::Client,
    commands::{Commands, ProjectOperations, ScriptOperations},
    errors::Result,
    handlers,
    settings::Settings,
};

/// Dispatches one parsed command to its handler, holding the client
/// and settings every handler needs.
pub struct Router {
    client: Client,
    settings: Settings,
}

impl Router {
    /// Creates a router over one client and its settings.
    pub fn new(client: Client, settings: Settings) -> Self {
        Self { client, settings }
    }

    /// Routes one command to its handler.
    ///
    /// # Errors
    /// Returns whatever the handler the command names returns.
    pub async fn route(&self, command: Commands) -> Result<()> {
        match command {
            Commands::Login => Ok(()), // Login is handled before this function is called
            Commands::Init {
                name,
                template,
                project_id,
            } => self.handle_init(name, template, project_id).await,
            Commands::Publish { directory } => self.handle_publish(directory).await,
            Commands::Dev {
                directory,
                worker_url,
            } => handlers::dev::handle(&self.client, &self.settings, &directory, &worker_url).await,
            Commands::Tail { target } => {
                handlers::tail::handle(&self.client, &self.settings, &target).await
            }
            Commands::Alias { script, sub } => {
                handlers::aliases::handle(&self.client, &script, &sub).await
            }
            Commands::Secret { project, sub } => {
                handlers::secrets::handle(&self.client, &project, &sub).await
            }
            Commands::Tokens { project, sub } => {
                handlers::tokens::handle(&self.client, &project, &sub).await
            }
            Commands::Scripts { project, page } => self.handle_list_scripts(project, page).await,
            Commands::Object { project, sub } => {
                handlers::objects::handle(&self.client, &project, sub).await
            }
            Commands::Shell { project } => handlers::shell::handle(&self.client, &project).await,
            Commands::Projects { page } => self.handle_list_projects(page).await,
            Commands::Project { sub } => self.handle_project(sub).await,
            Commands::Script { id, sub } => self.handle_script(id, sub).await,
            // Handled before authentication in main; unreachable here.
            Commands::Check { .. }
            | Commands::Test { .. }
            | Commands::Sql { .. }
            | Commands::Lsp => Ok(()),
        }
    }

    // Route to Init handler
    async fn handle_init(
        &self,
        name: String,
        template: Option<String>,
        project_id: Option<String>,
    ) -> Result<()> {
        handlers::init::handle(&self.client, &name, project_id, template).await
    }

    // Route to Publish handler
    async fn handle_publish(&self, directory: String) -> Result<()> {
        handlers::publish::handle(&self.client, &directory).await
    }

    // Route to List Scripts handler
    async fn handle_list_scripts(&self, project: String, page: Option<i64>) -> Result<()> {
        handlers::scripts::handle_list(&self.client, &project, page.unwrap_or(1) as f64).await
    }

    // Route to List Projects handler
    async fn handle_list_projects(&self, page: Option<i64>) -> Result<()> {
        handlers::projects::handle_list(&self.client, page.unwrap_or(1) as f64).await
    }

    // Route to Project management handler
    async fn handle_project(&self, operation: ProjectOperations) -> Result<()> {
        handlers::projects::handle_operation(&self.client, &operation).await
    }

    // Route to Script management handler
    async fn handle_script(&self, id: String, operation: ScriptOperations) -> Result<()> {
        handlers::scripts::handle_operation(&self.client, &id, &operation).await
    }

    // Route to Check handler
}
