use actias_common::logging::LogLine;
use actias_common::thiserror;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{Config, Runtime, redis};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::proto_script_service::{LogMessage, ScriptConfig};
use crate::{bundle::Bundle, proto_script_service::LiveScript};

/// Used to manage live script sessions.
pub struct LiveScriptManager {
    pool: deadpool_redis::Pool,
    /// Dedicated client for pub/sub subscriptions, which take over a whole
    /// connection and therefore cannot come from the pool.
    client: redis::Client,
}

/// Live script instance stored in Redis.
///
/// Each session is its own key so it can carry a TTL, with a per-script set
/// naming the sessions that belong to a script:
///
/// - `live:{script_id}:{session_id}` -> [`LiveScriptInstance`], expiring
/// - `live:{script_id}:sessions` -> set of session ids, expiring
#[derive(Serialize, Deserialize, Clone)]
pub struct LiveScriptInstance {
    pub script_config: ScriptConfig,
    pub bundle: Bundle,
}

#[derive(thiserror::Error, Debug)]
pub enum LiveScriptError {
    #[error("Livescript error: {0}")]
    Invalid(String),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("Stored session could not be read: {0}")]
    Corrupt(#[from] serde_json::Error),
}

impl From<LiveScriptError> for tonic::Status {
    fn from(err: LiveScriptError) -> Self {
        match err {
            LiveScriptError::Invalid(e) => tonic::Status::invalid_argument(e),
            LiveScriptError::Redis(e) => tonic::Status::internal(e.to_string()),
            LiveScriptError::Pool(e) => tonic::Status::internal(e.to_string()),
            LiveScriptError::Corrupt(e) => tonic::Status::internal(e.to_string()),
        }
    }
}

impl LiveScriptManager {
    /// How long a session outlives its last write.
    ///
    /// A live session belongs to a CLI that is still connected, so the client
    /// refreshes it by writing; if the client goes away the session expires
    /// instead of leaking.
    const SESSION_TTL: usize = 120;

    pub fn new(redis_url: &str) -> Self {
        let cfg = Config::from_url(redis_url);

        Self {
            pool: cfg
                .create_pool(Some(Runtime::Tokio1))
                .expect("redis pool could not be created from REDIS_URL"),
            client: redis::Client::open(redis_url)
                .expect("redis client could not be created from REDIS_URL"),
        }
    }

    /// Follows a log channel, yielding each line published to it.
    ///
    /// Lines that are not valid [`LogLine`] json are dropped rather than
    /// ending the stream, because one malformed publisher must not silence a
    /// tail.
    ///
    /// # Errors
    /// Returns [`LiveScriptError::Redis`] when the subscription cannot be
    /// established.
    pub async fn log_stream(
        &self,
        channel: &str,
    ) -> Result<impl Stream<Item = LogMessage> + Send + use<>, LiveScriptError> {
        let connection = self.client.get_async_connection().await?;
        let mut pubsub = connection.into_pubsub();
        pubsub.subscribe(channel).await?;

        Ok(pubsub.into_on_message().filter_map(|message| async move {
            let payload: String = message.get_payload().ok()?;
            let line: LogLine = serde_json::from_str(&payload).ok()?;

            Some(LogMessage {
                level: line.level,
                message: line.message,
                timestamp_ms: line.timestamp_ms,
            })
        }))
    }

    /// Key holding one session's bundle.
    fn session_key(script_id: &str, session_id: &str) -> String {
        format!("live:{script_id}:{session_id}")
    }

    /// Key holding the set of session ids belonging to a script.
    fn sessions_key(script_id: &str) -> String {
        format!("live:{script_id}:sessions")
    }

    /// Put a live script session, creating it or replacing it in place, and
    /// push its expiry out by [`LiveScriptManager::SESSION_TTL`].
    ///
    /// # Arguments
    /// * `script` - Session content. `session_id` names an existing session to
    ///   replace; without it a new session is created.
    ///
    /// # Returns
    /// Session ID which can be used to get the session bundle.
    ///
    /// # Errors
    /// Returns [`LiveScriptError::Invalid`] if the script id disagrees with the
    /// config, or if a supplied `session_id` is not a uuid.
    pub async fn put_session(&self, script: LiveScript) -> Result<Uuid, LiveScriptError> {
        if script.script_id != script.script_config.id {
            return Err(LiveScriptError::Invalid(
                "Script ID does not match script config".to_string(),
            ));
        }

        // Reusing the caller's id is what makes an update replace the session
        // the worker is serving, rather than orphaning it behind a new one.
        let session_id = match &script.session_id {
            Some(id) => Uuid::parse_str(id)
                .map_err(|_| LiveScriptError::Invalid(format!("'{id}' is not a session id")))?,
            None => Uuid::new_v4(),
        };

        let script_instance = LiveScriptInstance {
            script_config: script.script_config,
            bundle: script.bundle,
        };

        let session_key = Self::session_key(&script.script_id, &session_id.to_string());
        let sessions_key = Self::sessions_key(&script.script_id);

        let mut con = self.pool.get().await?;

        let _: () = con
            .set_ex(
                &session_key,
                serde_json::to_string(&script_instance)?,
                Self::SESSION_TTL,
            )
            .await?;

        let _: () = con.sadd(&sessions_key, session_id.to_string()).await?;
        let _: () = con.expire(&sessions_key, Self::SESSION_TTL).await?;

        Ok(session_id)
    }

    /// Delete a live script session.
    pub async fn delete_session(
        &self,
        script_id: &str,
        session_id: &str,
    ) -> Result<(), LiveScriptError> {
        let mut con = self.pool.get().await?;

        let _: () = con.del(Self::session_key(script_id, session_id)).await?;
        let _: () = con.srem(Self::sessions_key(script_id), session_id).await?;

        Ok(())
    }

    /// Delete all sessions for a script.
    pub async fn delete_script(&self, script_id: &str) -> Result<(), LiveScriptError> {
        let mut con = self.pool.get().await?;

        let sessions_key = Self::sessions_key(script_id);
        let session_ids: Vec<String> = con.smembers(&sessions_key).await?;

        // Members may name sessions that already expired, and deleting a key
        // that is gone is a no-op, so the set is a hint rather than an index.
        for session_id in session_ids {
            let _: () = con.del(Self::session_key(script_id, &session_id)).await?;
        }

        let _: () = con.del(&sessions_key).await?;

        Ok(())
    }

    /// Get a live script session.
    ///
    /// # Errors
    /// Returns [`LiveScriptError::Corrupt`] if the stored session cannot be
    /// read back, which means something other than this service wrote it.
    pub async fn get_session(
        &self,
        script_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveScriptInstance>, LiveScriptError> {
        let mut con = self.pool.get().await?;

        let script: Option<String> = con.get(Self::session_key(script_id, session_id)).await?;

        match script {
            Some(v) => Ok(Some(serde_json::from_str(&v)?)),
            None => Ok(None),
        }
    }
}

/// Container-backed tests for the session store.
///
/// These live here rather than in `tests/` because this crate is a binary and
/// has no library target for an integration test to import.
#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers_modules::redis::Redis;
    use testcontainers_modules::testcontainers::ContainerAsync;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;

    /// Starts a redis and returns a manager pointing at it.
    ///
    /// The container is returned alongside because dropping it stops redis.
    async fn redis() -> (ContainerAsync<Redis>, LiveScriptManager) {
        let container = Redis::default().start().await.expect("redis starts");
        let port = container
            .get_host_port_ipv4(6379)
            .await
            .expect("redis port is published");

        let manager = LiveScriptManager::new(&format!("redis://127.0.0.1:{port}"));
        (container, manager)
    }

    fn live_script(script_id: &str, session_id: Option<Uuid>, entry_point: &str) -> LiveScript {
        LiveScript {
            session_id: session_id.map(|id| id.to_string()),
            script_id: script_id.to_owned(),
            script_config: ScriptConfig {
                id: script_id.to_owned(),
                entry_point: entry_point.to_owned(),
                includes: vec![],
                ignore: vec![],
                capabilities: None,
            },
            bundle: Bundle {
                entry_point: entry_point.to_owned(),
                files: vec![],
            },
        }
    }

    #[tokio::test]
    async fn a_session_round_trips() {
        let (_container, manager) = redis().await;

        let session_id = manager
            .put_session(live_script("script-a", None, "main.lua"))
            .await
            .unwrap();

        let stored = manager
            .get_session("script-a", &session_id.to_string())
            .await
            .unwrap()
            .expect("session was stored");

        assert_eq!(stored.script_config.entry_point, "main.lua");
    }

    #[tokio::test]
    async fn putting_a_known_session_replaces_it_rather_than_orphaning_it() {
        let (_container, manager) = redis().await;

        let session_id = manager
            .put_session(live_script("script-b", None, "first.lua"))
            .await
            .unwrap();

        let updated = manager
            .put_session(live_script("script-b", Some(session_id), "second.lua"))
            .await
            .unwrap();

        // The caller keeps its session id, so the worker keeps serving the
        // session it already knows about.
        assert_eq!(updated, session_id);

        let stored = manager
            .get_session("script-b", &session_id.to_string())
            .await
            .unwrap()
            .expect("session still exists under the original id");
        assert_eq!(stored.script_config.entry_point, "second.lua");

        // An update must not leave a second session behind.
        let mut con = manager.pool.get().await.unwrap();
        let sessions: Vec<String> = con
            .smembers(LiveScriptManager::sessions_key("script-b"))
            .await
            .unwrap();
        assert_eq!(sessions, vec![session_id.to_string()]);
    }

    #[tokio::test]
    async fn a_session_expires_on_its_own() {
        let (_container, manager) = redis().await;

        let session_id = manager
            .put_session(live_script("script-c", None, "main.lua"))
            .await
            .unwrap();

        // Redis hash fields cannot expire, which is why sessions are their own
        // keys; without a ttl a disconnected client leaks one forever.
        let mut con = manager.pool.get().await.unwrap();
        let ttl: i64 = con
            .ttl(LiveScriptManager::session_key(
                "script-c",
                &session_id.to_string(),
            ))
            .await
            .unwrap();

        assert!(
            ttl > 0 && ttl <= LiveScriptManager::SESSION_TTL as i64,
            "expected a live ttl, got {ttl}"
        );
    }

    #[tokio::test]
    async fn deleting_a_session_removes_only_that_session() {
        let (_container, manager) = redis().await;

        let first = manager
            .put_session(live_script("script-d", None, "a.lua"))
            .await
            .unwrap();
        let second = manager
            .put_session(live_script("script-d", None, "b.lua"))
            .await
            .unwrap();

        manager
            .delete_session("script-d", &first.to_string())
            .await
            .unwrap();

        assert!(
            manager
                .get_session("script-d", &first.to_string())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            manager
                .get_session("script-d", &second.to_string())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn deleting_a_script_removes_every_session_it_has() {
        let (_container, manager) = redis().await;

        let first = manager
            .put_session(live_script("script-e", None, "a.lua"))
            .await
            .unwrap();
        let second = manager
            .put_session(live_script("script-e", None, "b.lua"))
            .await
            .unwrap();

        manager.delete_script("script-e").await.unwrap();

        for session_id in [first, second] {
            assert!(
                manager
                    .get_session("script-e", &session_id.to_string())
                    .await
                    .unwrap()
                    .is_none(),
                "{session_id} survived the script delete"
            );
        }
    }

    #[tokio::test]
    async fn a_session_id_that_is_not_a_uuid_is_rejected() {
        let (_container, manager) = redis().await;

        let mut script = live_script("script-f", None, "main.lua");
        script.session_id = Some("not-a-uuid".to_owned());

        let error = manager.put_session(script).await.unwrap_err();
        assert!(matches!(error, LiveScriptError::Invalid(_)), "{error:?}");
    }
}
