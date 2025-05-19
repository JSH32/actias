use clap::{Parser, Subcommand};

/// Actias CLI for interacting with the actias API.
#[derive(Parser, Debug)]
#[command(propagate_version = true)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 🔑 Login to an Actias account.
    Login,
    /// 📜 Initialize a new sample project
    Init {
        /// Folder name of the new project
        name: String,
        /// Template name
        template: Option<String>,
        /// Id of the project to create the script under.
        project_id: Option<String>,
    },
    /// 🚀 Publish a new revision of the project
    Publish {
        /// Directory of project to publish
        directory: String,
    },
    /// 📁 List projects
    Projects { page: Option<i64> },
    /// 📜 Manage a project
    Project {
        /// Project to manage.
        id: String,
        #[clap(subcommand)]
        sub: ProjectOperations,
    },
    /// 📑 List scripts
    Scripts { project: String, page: Option<i64> },
    /// 📜 Manage a script
    Script {
        /// Script to manage.
        id: String,
        #[clap(subcommand)]
        sub: ScriptOperations,
    },
    /// Check a project config and generate definitions.
    Check {
        /// Directory of project
        directory: String,
    },
}

#[derive(Parser, Debug)]
pub enum ProjectOperations {
    /// 🚮 Delete a project and all resources.
    Delete,
}

#[derive(Parser, Debug)]
pub enum ScriptOperations {
    /// 🚮 Delete a script and all revisions.
    Delete,
    /// 🖊️ Manage revisions for this script
    Revisions {
        #[clap(subcommand)]
        sub: RevisionCommands,
    },
    /// Clone the most recent revision to filesystem.
    Clone { path: Option<String> },
}

#[derive(Parser, Debug)]
pub enum RevisionCommands {
    /// 🚮 Delete a revision, this will try to set the script's revision to the most recent
    Delete { revision_id: String },
    /// 📑 List revisions
    List { page: Option<i64> },
    /// 📦 Set script to use a specific revision.
    Set { revision_id: String },
    /// Clone a revision to filesystem.
    Clone {
        /// This will get the current active revision if not provided.
        revision_id: Option<String>,
        #[clap(short, long)]
        path: Option<String>,
    },
}
