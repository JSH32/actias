use crate::{
    client::Client,
    commands::{Commands, ProjectOperations, ScriptOperations},
    errors::Result,
    handlers,
};

/// Router for dispatching commands to their handlers
pub struct Router {
    client: Client,
}

impl Router {
    /// Create a new router with the given client
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Route command to appropriate handler
    pub async fn route(&self, command: Commands) -> Result<()> {
        match command {
            Commands::Login => Ok(()), // Login is handled before this function is called
            Commands::Init {
                name,
                template,
                project_id,
            } => self.handle_init(name, template, project_id).await,
            Commands::Publish { directory } => self.handle_publish(directory).await,
            Commands::Scripts { project, page } => self.handle_list_scripts(project, page).await,
            Commands::Projects { page } => self.handle_list_projects(page).await,
            Commands::Project { id, sub } => self.handle_project(id, sub).await,
            Commands::Script { id, sub } => self.handle_script(id, sub).await,
            Commands::Check { directory } => self.handle_check(directory),
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
    async fn handle_project(&self, id: String, operation: ProjectOperations) -> Result<()> {
        handlers::projects::handle_operation(&self.client, &id, &operation).await
    }

    // Route to Script management handler
    async fn handle_script(&self, id: String, operation: ScriptOperations) -> Result<()> {
        handlers::scripts::handle_operation(&self.client, &id, &operation).await
    }

    // Route to Check handler
    fn handle_check(&self, directory: String) -> Result<()> {
        handlers::check::handle(&directory)
    }
}
