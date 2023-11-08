use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::types::{
    chrono::{DateTime, Utc},
    Uuid,
};

use crate::proto_script_service::Script;

#[derive(Serialize, Deserialize, Clone, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScriptConfig {
    pub id: Uuid,
    pub entry_point: String,
    pub includes: Vec<String>,
    pub ignore: Vec<String>,
}

impl TryInto<ScriptConfig> for crate::proto_script_service::ScriptConfig {
    type Error = uuid::Error;

    fn try_into(self) -> Result<ScriptConfig, Self::Error> {
        Ok(ScriptConfig {
            id: Uuid::from_str(&self.id)?,
            entry_point: self.entry_point,
            includes: self.includes,
            ignore: self.ignore,
        })
    }
}

impl Into<crate::proto_script_service::ScriptConfig> for ScriptConfig {
    fn into(self) -> crate::proto_script_service::ScriptConfig {
        crate::proto_script_service::ScriptConfig {
            id: self.id.to_string(),
            entry_point: self.entry_point,
            includes: self.includes,
            ignore: self.ignore,
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
    pub revision_id: Uuid,
    pub content: Vec<u8>,
    pub file_name: String,
    pub file_path: String,
}

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct DbScript {
    pub id: Uuid,
    pub project_id: Uuid,
    pub public_identifier: String,
    pub last_updated: DateTime<Utc>,
    pub current_revision: Option<Uuid>,
}

impl Into<Script> for DbScript {
    fn into(self) -> Script {
        Script {
            id: self.id.to_string(),
            project_id: self.project_id.to_string(),
            public_identifier: self.public_identifier,
            last_updated: self.last_updated.to_string(),
            current_revision_id: self.current_revision.map(|v| v.to_string()),
        }
    }
}
