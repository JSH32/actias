use serde::{Deserialize, Serialize};
use sqlx::types::{
    chrono::{DateTime, Utc},
    Uuid,
};

use crate::proto_script_service::Script;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub id: Uuid,
    pub entry_point: String,
    pub includes: Vec<String>,
}

#[derive(sqlx::FromRow)]
pub struct DbRevision {
    pub id: Uuid,
    pub created: DateTime<Utc>,
    pub script_id: Uuid,
    pub entry_point: String,
    pub project_config: sqlx::types::Json<ProjectConfig>,
}

#[derive(sqlx::FromRow)]
pub struct DbFile {
    pub revision_id: Uuid,
    pub content: Vec<u8>,
    pub file_name: String,
    pub file_path: String,
}

#[derive(sqlx::FromRow, Clone)]
pub struct DbScript {
    pub id: Uuid,
    pub public_identifier: String,
    pub last_updated: DateTime<Utc>,
    pub current_revision: Option<Uuid>,
}

impl Into<Script> for DbScript {
    fn into(self) -> Script {
        Script {
            id: self.id.to_string(),
            public_identifier: self.public_identifier,
            last_updated: self.last_updated.to_string(),
            current_revision_id: self.current_revision.map(|v| v.to_string()),
        }
    }
}
