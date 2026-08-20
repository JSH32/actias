//! Script log output, published fire-and-forget to a redis channel where
//! [`actias-common::logging`] subscribers (the live tail, `actias tail`)
//! pick it up.

use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use actias_common::logging::LogLine;
use actias_common::tracing::{debug, trace};
use mlua::LuaSerdeExt;
use redis::AsyncCommands;
use std::time::{SystemTime, UNIX_EPOCH};

/// The level names scripts may log at, which are also the function names.
const LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];

/// Publishes log lines to one channel, without waiting on delivery.
#[derive(Clone)]
pub struct LogPublisher {
    connection: redis::aio::ConnectionManager,
    channel: String,
}

impl LogPublisher {
    pub fn new(connection: redis::aio::ConnectionManager, channel: String) -> Self {
        Self {
            connection,
            channel,
        }
    }

    /// Publishes one line and returns immediately.
    ///
    /// Logging must never slow a script down or fail a request, so delivery
    /// is a detached task and a failed publish is only traced. Public so
    /// the platform can put its own lines on a stream (a live session's
    /// failure belongs in front of its developer), not only script `log.*`
    /// calls.
    pub fn publish(&self, level: &str, message: String) {
        let line = LogLine {
            level: level.to_owned(),
            message,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        };

        let Ok(payload) = serde_json::to_string(&line) else {
            return;
        };

        let mut connection = self.connection.clone();
        let channel = self.channel.clone();
        tokio::spawn(async move {
            if let Err(error) = connection.publish::<_, _, ()>(&channel, payload).await {
                trace!(%error, "log line dropped");
            }
        });
    }
}

/// Log output.
pub struct LogExtension {
    /// Absent when the worker has nowhere to send lines, in which case they
    /// still land in the worker's own tracing.
    pub publisher: Option<LogPublisher>,
}

impl LuaExtension for LogExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "log",
            description: "Log output, streamed to live sessions and tails",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let log = lua.create_table()?;

        for level in LEVELS {
            let publisher = self.publisher.clone();

            log.set(
                level,
                lua.create_function(move |lua, value: mlua::Value| {
                    let message = render(lua, value)?;

                    debug!(level, message, "script log");

                    if let Some(publisher) = &publisher {
                        publisher.publish(level, message);
                    }

                    Ok(())
                })?,
            )?;
        }

        Ok(mlua::Value::Table(log))
    }
}

/// Renders a lua value into one log line: strings verbatim, everything else
/// as json so tables stay readable.
fn render(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<String> {
    Ok(match &value {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let json: serde_json::Value = lua.from_value(value)?;
            json.to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::extension::LuaExtension;

    /// A lua state with only the log extension, publishing nowhere.
    fn lua_with_log() -> mlua::Lua {
        let lua = mlua::Lua::new();
        let log = LogExtension { publisher: None }
            .create_extension(&lua)
            .expect("extension builds");
        lua.globals().set("log", log).expect("global sets");
        lua
    }

    #[test]
    fn every_level_is_callable_with_strings_and_tables() {
        let lua = lua_with_log();

        lua.load(
            r#"
            log.debug("plain")
            log.info({ nested = { value = 1 } })
            log.warn(42)
            log.error(true)
        "#,
        )
        .exec()
        .expect("all levels accept values");
    }
}
