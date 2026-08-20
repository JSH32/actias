//! The cold-alarm sweep: a hibernated object's alarm fires from its own
//! task, but an object that was resident when its node died has no task
//! to wake it. Alarms are mirrored into the placement store, so the sweep
//! is one indexed query any live node can serve: every due row names an
//! object, waking it is a claim, and the claim race arbitrates which node
//! actually does. The spawn path re-arms the persisted alarm and fires
//! past-due ones immediately, so waking IS the whole fix: a dead node's
//! timers fire from a survivor without anyone touching the object.
//!
//! The registry mirror is asynchronous, so a crash can lose a write. The
//! local files are the durable truth: a one-time boot scan re-mirrors
//! every alarm this node's disk holds, which is exactly the set whose
//! mirror this node could have lost.

use std::path::{Path, PathBuf};
use std::time::Duration;

use actias_common::tracing::{debug, warn};
use actias_worker_core::extensions::objects::unix_now_ms;
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::proto::node_registry::{DueAlarmsRequest, SetAlarmRequest};
use actias_worker_core::storage::SqliteStorage;

use crate::routing::{ObjectRouting, owner_prepared};
use crate::server::AppState;

/// Largest batch one sweep drains; the next sweep takes the rest.
const SWEEP_BATCH: u32 = 256;

/// Every persisted alarm in this node's data files: (own key, due ms).
///
/// Pure scan, read-only opens, no waking. Files without an alarm, or
/// unreadable ones, are simply skipped. Runs once at boot to re-mirror
/// what a crash may have kept out of the registry.
pub fn scan_alarms(data_dir: &Path) -> Vec<(String, i64)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return found;
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
        if !own_key.is_empty() {
            found.push((own_key, due_ms));
        }
    }

    found
}

/// Re-mirrors every locally persisted alarm into the registry; best
/// effort, once. An alarm this misses still fires through its own file
/// the next time the object is resident here.
async fn mirror_local_alarms(state: &AppState) {
    let data_dir = state.object_data_dir.clone();
    let Ok(found) = tokio::task::spawn_blocking(move || scan_alarms(&data_dir)).await else {
        return;
    };

    for (own_key, due_ms) in found {
        let Some(key) = ObjectKey::parse(&own_key) else {
            continue;
        };
        if let Err(error) = state
            .registry
            .clone()
            .set_alarm(SetAlarmRequest {
                object_id: key.object_id(),
                own_key: own_key.clone(),
                due_ms,
            })
            .await
        {
            warn!(error = %error, own_key, "boot alarm mirror failed");
        }
    }
}

/// Runs forever; spawn it and forget it.
pub async fn run(state: AppState, every: Duration) {
    mirror_local_alarms(&state).await;

    loop {
        tokio::time::sleep(every).await;

        let due = state
            .registry
            .clone()
            .due_alarms(DueAlarmsRequest {
                now_ms: unix_now_ms(),
                limit: SWEEP_BATCH,
            })
            .await;
        let due = match due {
            Ok(response) => response.into_inner().alarms,
            // The registry being down pauses sweeping, nothing more; the
            // next tick asks again.
            Err(error) => {
                debug!(error = %error, "alarm sweep query failed");
                continue;
            }
        };

        for row in due {
            // Resident here: its own task is already waiting on the
            // alarm clock; the row clears when it fires.
            if state.objects.is_resident(&row.own_key).await {
                continue;
            }
            match wake(&state, &row.own_key).await {
                Ok(()) => debug!(own_key = row.own_key, "cold object woken for its due alarm"),
                // A live incumbent means the alarm is that node's
                // business (or the clear is still in flight): quiet skip,
                // not a failure.
                Err(WakeError::Elsewhere(holder)) => {
                    debug!(own_key = row.own_key, holder, "due alarm homed elsewhere")
                }
                // Next sweep retries; a missing script (deleted since)
                // stays noisy until its row is cleaned up, on purpose.
                Err(WakeError::Other(error)) => {
                    warn!(%error, own_key = row.own_key, "cold object could not be woken")
                }
            }
        }
    }
}

/// Why one wake did not happen here.
enum WakeError {
    /// A live incumbent holds the lease; its node owns the alarm.
    Elsewhere(String),
    Other(String),
}

/// Makes one object resident under its owner's current revision; its own
/// task does the rest.
async fn wake(state: &AppState, own_key: &str) -> Result<(), WakeError> {
    let key = ObjectKey::parse(own_key)
        .ok_or_else(|| WakeError::Other(format!("'{own_key}' is not an object key")))?;

    // The owner's current revision is what a wake runs, exactly like any
    // other touch; the routing resolves it again internally, off the same
    // cache.
    let owner = owner_prepared(state, &key)
        .await
        .map_err(WakeError::Other)?;

    ObjectRouting::new(state, owner)
        .resolve_handle(&key)
        .await
        .map(|_| ())
        .map_err(|error| match error {
            crate::routing::ResolveError::Elsewhere(holder) => WakeError::Elsewhere(holder),
            crate::routing::ResolveError::Other(error) => WakeError::Other(error),
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
    fn the_scan_finds_every_alarm_with_its_due_time() {
        let dir = tempfile::tempdir().expect("tempdir");

        // The boot mirror wants them ALL, due and future alike: the
        // registry decides dueness, the file only remembers.
        file_with_alarm(dir.path(), "due.db", "script-1/Keeper/due", -5_000);
        file_with_alarm(dir.path(), "future.db", "script-1/Keeper/future", 60_000);
        // A file with no alarm at all.
        SqliteStorage::open(&dir.path().join("plain.db")).expect("opens");
        // Not a database file.
        std::fs::write(dir.path().join("note.txt"), "ignore me").expect("writes");

        let mut found = scan_alarms(dir.path());
        found.sort();
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].0, "script-1/Keeper/due");
        assert!(found[0].1 <= unix_now_ms());
        assert_eq!(found[1].0, "script-1/Keeper/future");
        assert!(found[1].1 > unix_now_ms());
    }

    #[test]
    fn an_empty_or_missing_dir_scans_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(scan_alarms(dir.path()).is_empty());
        assert!(scan_alarms(&dir.path().join("nope")).is_empty());
    }
}
