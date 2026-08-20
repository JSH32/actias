//! The cold-alarm sweep: a hibernated object's alarm fires from its own
//! task, but an object that was resident when the node died has no task
//! to wake it. This sweep scans the node's own data files for persisted
//! alarms that are due with no resident vm and simply makes the object
//! resident; the spawn path re-arms the alarm and fires past-due ones
//! immediately, so waking IS the whole fix.
//!
//! Deliberately node-local: objects only live where their files are, so
//! scanning our own disk covers every alarm this node can be responsible
//! for. A placement-store alarm registry becomes necessary exactly when
//! WAL shipping lets objects move; it arrives with that work.

use std::path::{Path, PathBuf};
use std::time::Duration;

use actias_common::tracing::{debug, warn};
use actias_worker_core::extensions::objects::unix_now_ms;
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::storage::SqliteStorage;

use crate::routing::{ObjectRouting, owner_prepared};
use crate::server::AppState;

/// Object keys (scope/class/name) whose persisted alarm is due.
///
/// Pure scan over the data dir; read-only opens, no waking. Files without
/// an alarm, or unreadable ones, are simply skipped.
pub fn scan_due(data_dir: &Path) -> Vec<String> {
    let mut due = Vec::new();
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return due;
    };

    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Ok(mut storage) = SqliteStorage::open_read_only(&path) else {
            continue;
        };
        let Ok(Some((due_ms, _class, _name, own_key))) = storage.peek_alarm() else {
            continue;
        };
        if due_ms <= unix_now_ms() && !own_key.is_empty() {
            due.push(own_key);
        }
    }

    due
}

/// Runs forever; spawn it and forget it.
pub async fn run(state: AppState, every: Duration) {
    loop {
        tokio::time::sleep(every).await;

        let data_dir = state.object_data_dir.clone();
        let found = tokio::task::spawn_blocking(move || scan_due(&data_dir)).await;
        let Ok(found) = found else { continue };

        for own_key in found {
            if state.objects.is_resident(&own_key).await {
                continue;
            }
            if let Err(error) = wake(&state, &own_key).await {
                // Next sweep retries; a missing script (deleted since)
                // stays noisy until its file is cleaned up, on purpose.
                warn!(%error, own_key, "cold object could not be woken");
            } else {
                debug!(own_key, "cold object woken for its due alarm");
            }
        }
    }
}

/// Makes one object resident under its owner's current revision; its own
/// task does the rest.
async fn wake(state: &AppState, own_key: &str) -> Result<(), String> {
    let key =
        ObjectKey::parse(own_key).ok_or_else(|| format!("'{own_key}' is not an object key"))?;

    // The owner's current revision is what a wake runs, exactly like any
    // other touch; the routing resolves it again internally, off the same
    // cache.
    let owner = owner_prepared(state, &key).await?;

    ObjectRouting::new(state, owner)
        .resolve_handle(&key)
        .await
        .map(|_| ())
        .map_err(|error| match error {
            // Someone else holds it now; their sweep is responsible.
            crate::routing::ResolveError::Elsewhere(holder) => {
                format!("homed on {holder}; not ours to wake")
            }
            crate::routing::ResolveError::Other(error) => error,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data file holding an alarm `offset_ms` from now for `own_key`.
    fn file_with_alarm(dir: &Path, name: &str, own_key: &str, offset_ms: i64) {
        let mut storage = SqliteStorage::open(&dir.join(name)).expect("opens");
        storage
            .save_alarm(unix_now_ms() + offset_ms, "Keeper", "watchdog", own_key)
            .expect("saves");
    }

    #[test]
    fn the_scan_finds_due_alarms_and_only_those() {
        let dir = tempfile::tempdir().expect("tempdir");

        file_with_alarm(dir.path(), "due.db", "script-1/Keeper/due", -5_000);
        file_with_alarm(dir.path(), "future.db", "script-1/Keeper/future", 60_000);
        // A file with no alarm at all.
        SqliteStorage::open(&dir.path().join("plain.db")).expect("opens");
        // Not a database file.
        std::fs::write(dir.path().join("note.txt"), "ignore me").expect("writes");

        let due = scan_due(dir.path());
        assert_eq!(due, vec!["script-1/Keeper/due".to_owned()]);
    }

    #[test]
    fn an_empty_or_missing_dir_scans_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(scan_due(dir.path()).is_empty());
        assert!(scan_due(&dir.path().join("nope")).is_empty());
    }
}
