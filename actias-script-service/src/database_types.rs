use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::types::{
    Uuid,
    chrono::{DateTime, Utc},
};

use crate::proto_script_service::Script;

#[derive(Serialize, Deserialize, Clone, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScriptConfig {
    pub id: Uuid,
    pub entry_point: String,
    pub includes: Vec<String>,
    pub ignore: Vec<String>,
    /// The script's declared capability contract, derived from its code at
    /// publish; absent on revisions stored before extraction existed.
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
}

/// What a script declared at its top level.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Capabilities {
    pub kv: Vec<String>,
    pub events: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub objects: Vec<String>,
    #[serde(default)]
    pub databases: Vec<String>,
    #[serde(default)]
    pub queues: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
    #[serde(default)]
    pub workflow_steps: Vec<String>,
    #[serde(default)]
    pub publishes: Vec<String>,
    #[serde(default)]
    pub lifecycle: Vec<String>,
    #[serde(default)]
    pub connections: Vec<String>,
}

impl From<crate::proto_script_service::Capabilities> for Capabilities {
    fn from(val: crate::proto_script_service::Capabilities) -> Self {
        Capabilities {
            kv: val.kv,
            events: val.events,
            secrets: val.secrets,
            objects: val.objects,
            databases: val.databases,
            queues: val.queues,
            workflows: val.workflows,
            workflow_steps: val.workflow_steps,
            publishes: val.publishes,
            lifecycle: val.lifecycle,
            connections: val.connections,
        }
    }
}

impl From<Capabilities> for crate::proto_script_service::Capabilities {
    fn from(val: Capabilities) -> Self {
        crate::proto_script_service::Capabilities {
            kv: val.kv,
            events: val.events,
            secrets: val.secrets,
            objects: val.objects,
            databases: val.databases,
            queues: val.queues,
            workflows: val.workflows,
            workflow_steps: val.workflow_steps,
            publishes: val.publishes,
            lifecycle: val.lifecycle,
            connections: val.connections,
        }
    }
}

impl TryInto<ScriptConfig> for crate::proto_script_service::ScriptConfig {
    type Error = uuid::Error;

    fn try_into(self) -> Result<ScriptConfig, Self::Error> {
        Ok(ScriptConfig {
            id: Uuid::from_str(&self.id)?,
            entry_point: self.entry_point,
            includes: self.includes,
            ignore: self.ignore,
            capabilities: self.capabilities.map(Into::into),
        })
    }
}

impl From<ScriptConfig> for crate::proto_script_service::ScriptConfig {
    fn from(val: ScriptConfig) -> Self {
        crate::proto_script_service::ScriptConfig {
            id: val.id.to_string(),
            entry_point: val.entry_point,
            includes: val.includes,
            ignore: val.ignore,
            capabilities: val.capabilities.map(Into::into),
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct DbRevision {
    pub id: Uuid,
    pub created: DateTime<Utc>,
    pub script_id: Uuid,
    pub entry_point: String,
    pub script_config: sqlx::types::Json<ScriptConfig>,
}

#[derive(sqlx::FromRow)]
pub struct DbFile {
    pub file_path: String,
    pub hash: String,
    pub size: i64,
    pub content_type: String,
    pub kind: i16,
}

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct DbScript {
    pub id: Uuid,
    pub project_id: Uuid,
    pub public_identifier: String,
    pub last_updated: DateTime<Utc>,
    pub current_revision: Option<Uuid>,
}

impl From<DbScript> for Script {
    fn from(val: DbScript) -> Self {
        Script {
            id: val.id.to_string(),
            project_id: val.project_id.to_string(),
            public_identifier: val.public_identifier,
            last_updated: val.last_updated.to_string(),
            current_revision_id: val.current_revision.map(|v| v.to_string()),
        }
    }
}
