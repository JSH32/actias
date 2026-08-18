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
    /// 🔁 Run a live development session: every save updates a live URL
    Dev {
        /// Directory of the project to develop
        directory: String,
        /// Base URL of the worker serving live sessions
        #[clap(long, default_value = "http://127.0.0.1:3002")]
        worker_url: String,
    },
    /// 📡 Stream a published script's log lines
    Tail {
        /// A project directory or a script id
        target: String,
    },
    /// 🏷️ Manage a script's environment aliases (staging, prod)
    Alias {
        /// Script the aliases belong to.
        script: String,
        #[clap(subcommand)]
        sub: AliasOperations,
    },
    /// 🔐 Manage a project's secrets
    Secret {
        /// Project the secrets belong to.
        project: String,
        #[clap(subcommand)]
        sub: SecretOperations,
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
    /// 🧪 Run tests/*.lua on the local runtime with in-memory fakes.
    Test {
        /// Directory of project; defaults to the current one.
        directory: Option<String>,
    },
}

#[derive(Parser, Debug)]
pub enum AliasOperations {
    /// 🏷️ Point an alias at a revision; creating and moving are the same call.
    Set { name: String, revision_id: String },
    /// 📑 List aliases and the revisions they serve.
    List,
}

#[derive(Parser, Debug)]
pub enum SecretOperations {
    /// 🔏 Set a secret, prompting for the value when not given.
    Put {
        name: String,
        /// Value; prompted for (hidden) when omitted.
        value: Option<String>,
    },
    /// 📑 List secret names. Values are never shown.
    List,
    /// 🚮 Delete a secret.
    Delete { name: String },
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
