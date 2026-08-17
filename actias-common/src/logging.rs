//! The log line contract between the worker (publisher) and script-service
//! (subscriber): one redis pub/sub channel per log target, carrying
//! [`LogLine`] as json. Both sides build channel names and payloads from
//! here, so they cannot drift apart.

use serde::{Deserialize, Serialize};

/// One log line a running script emitted.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogLine {
    /// Level name: `debug`, `info`, `warn` or `error`.
    pub level: String,
    /// The rendered line.
    pub message: String,
    /// Milliseconds since the unix epoch, stamped by the worker.
    pub timestamp_ms: i64,
}

/// Channel carrying one live session's log lines.
pub fn live_log_channel(session_id: &str) -> String {
    format!("livelog:{session_id}")
}

/// Channel carrying a script's production log lines.
pub fn script_log_channel(script_id: &str) -> String {
    format!("scriptlog:{script_id}")
}
