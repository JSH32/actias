use actias_common::thiserror;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{redis, Config, Runtime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::proto_script_service::ScriptConfig;
use crate::{bundle::Bundle, proto_script_service::LiveScript};

/// Used to manage live script sessions.
pub struct LiveScriptManager {
    pool: deadpool_redis::Pool,
}

/// Live script instance stored in Redis.
///
/// Stored in redis as:
///
/// Primary Key: script_id
///   Hash Key: session_id -> LiveScriptInstance
#[derive(Serialize, Deserialize, Clone)]
pub struct LiveScriptInstance {
    pub script_config: ScriptConfig,
    pub bundle: Bundle,
}

#[derive(thiserror::Error, Debug)]
pub enum LiveScriptError {
    #[error("Livescript error: {0}")]
    LiveScriptError(String),
    #[error("Redis error: {0}")]
    RedisError(#[from] redis::RedisError),
    #[error("Pool error: {0}")]
    PoolError(#[from] deadpool_redis::PoolError),
}

impl From<LiveScriptError> for tonic::Status {
    fn from(err: LiveScriptError) -> Self {
        match err {
            LiveScriptError::LiveScriptError(e) => tonic::Status::invalid_argument(e),
            LiveScriptError::RedisError(e) => tonic::Status::internal(e.to_string()),
            LiveScriptError::PoolError(e) => tonic::Status::internal(e.to_string()),
        }
    }
}

impl LiveScriptManager {
    pub fn new(redis_url: &str) -> Self {
        let cfg = Config::from_url(redis_url);

        Self {
            pool: cfg.create_pool(Some(Runtime::Tokio1)).unwrap(),
        }
    }

    /// Put a new live script session.
    /// This will update the session if `session_id` is provided.
    ///
    /// # Arguments
    ///
    /// # Returns
    /// Session ID which can be used to get the session bundle.
    pub async fn put_session(&self, script: LiveScript) -> Result<Uuid, LiveScriptError> {
        if script.script_id != script.script_config.id {
            return Err(LiveScriptError::LiveScriptError(
                "Script ID does not match script config".to_string(),
            ));
        }

        let session_id = uuid::Uuid::new_v4();
        let script_instance = LiveScriptInstance {
            script_config: script.script_config,
            bundle: script.bundle,
        };

        let mut con = self.pool.get().await?;

        let _: () = con
            .hset(
                &script.script_id,
                &session_id.to_string(),
                serde_json::to_string(&script_instance).unwrap(),
            )
            .await?;

        Ok(session_id)
    }

    /// Delete a live script session.
    pub async fn delete_session(
        &self,
        script_id: &str,
        session_id: &str,
    ) -> Result<(), LiveScriptError> {
        let mut con = self.pool.get().await?;

        let _: () = con.hdel(script_id, session_id).await?;

        Ok(())
    }

    /// Delete all sessions for a script.
    pub async fn delete_script(&self, script_id: &str) -> Result<(), LiveScriptError> {
        let mut con = self.pool.get().await?;

        let _: () = con.del(script_id).await?;

        Ok(())
    }

    /// Get a live script session.
    pub async fn get_session(
        &self,
        script_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveScriptInstance>, LiveScriptError> {
        let mut con = self.pool.get().await?;

        let script: Option<String> = con.hget(script_id, session_id).await?;

        match script {
            Some(v) => {
                let script_instance: LiveScriptInstance = serde_json::from_str(&v).unwrap();
                Ok(Some(script_instance))
            }
            None => Ok(None),
        }
    }
}
