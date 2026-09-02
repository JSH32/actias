//! The crash-scoped sweep: repairing exactly what a dead node held.
//!
//! A node that exits cleanly drains, flushing its shipper and syncer to
//! zero, so it leaves nothing owed. A node that dies can leave a gap:
//! the flight that settled a write landed the object's manifest, but the
//! row it carries never reached a delta, because the syncer died with
//! the process. That row is missing from the index, which is the one
//! failure this design refuses.
//!
//! Scoped to the crash, which is the whole point. The periodic
//! reconciliation walks every class on every node and so multiplies by
//! tenant count; this walks exactly the objects one dead node held and
//! costs O(objects the crash touched). It is the event-driven half, and
//! reconciliation becomes the belt over its braces.
//!
//! Manifest reads only. Every shipping manifest carries the object's
//! settled row, so this is a metadata copy: no restores, no leases, no
//! object files opened, nothing woken.

use std::collections::HashMap;

use actias_worker_core::directory::delta;
use actias_worker_core::directory::repair::{self, Carried};
use actias_worker_core::identity::ObjectKey;

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// What one taken departure repaired.
#[derive(Debug, Default, PartialEq)]
pub struct Swept {
    /// The dead node, empty when there was nothing to take.
    pub node_id: String,
    /// Identities the departure named.
    pub held: usize,
    /// Rows recovered from their manifests.
    pub rows: usize,
    /// Classes a delta was written for.
    pub classes: usize,
}

/// Takes one departure and repairs the rows its objects owe.
///
/// # Errors
/// Returns the registry's or the store's message. A single object's
/// unreadable manifest is skipped with a warning: a sweep that repairs
/// most of a crash is worth more than one that repairs none.
pub async fn sweep_once(state: &AppState) -> Result<Swept, String> {
    let departure = state
        .registry
        .clone()
        .take_departure(())
        .await
        .map_err(|e| e.to_string())?
        .into_inner();

    // Nothing owed: the common case, one cheap query.
    if departure.node_id.is_empty() {
        return Ok(Swept::default());
    }

    let mut summary = Swept {
        node_id: departure.node_id.clone(),
        held: departure.instances.len(),
        ..Swept::default()
    };

    // Grouped by class, because a delta is written under one class's
    // prefix and a dead node may have held objects of several.
    let mut by_class: HashMap<ClassKey, Vec<Carried>> = HashMap::new();
    for instance in departure.instances {
        let object_id =
            ObjectKey::received(&instance.scope_id, &instance.class, &instance.name).object_id();

        let manifest = match state.object_store.manifest(&object_id).await {
            Ok(Some(manifest)) => manifest,
            // Nothing ever settled for it, so the crash cost no row.
            Ok(None) => continue,
            Err(error) => {
                actias_common::tracing::warn!(
                    class = %instance.class,
                    name = %instance.name,
                    %error,
                    "a manifest could not be read during a crash sweep"
                );
                continue;
            }
        };

        by_class
            .entry(ClassKey {
                scope_id: instance.scope_id,
                class: instance.class,
            })
            .or_default()
            .push(Carried {
                object_id,
                name: instance.name,
                epoch: manifest.epoch,
                deleted: manifest.deleted,
                row: manifest.directory,
            });
    }

    for (class, carried) in by_class {
        let repaired = repair::rows_from_manifests(carried);
        if repaired.rows.is_empty() {
            continue;
        }
        summary.rows += repaired.rows.len();

        // No declaration: the sweep copies rows out of manifests
        // without knowing which publish derived them, and guessing
        // would let a repair rewrite a class's field set.
        let bytes = delta::encode(&repaired.rows, None, &state.object_data_dir)?;
        let name = blake3::hash(&bytes).to_hex().to_string();
        state
            .object_store
            .put_directory_delta(&class.scope_id, &class.class, &name, bytes)
            .await?;
        summary.classes += 1;
    }

    actias_common::tracing::info!(
        node = %summary.node_id,
        held = summary.held,
        rows = summary.rows,
        classes = summary.classes,
        "a crash-scoped sweep repaired a dead node's directory rows"
    );
    Ok(summary)
}

/// Takes departures as they appear, forever.
///
/// Every node runs this; the take is atomic and deleting, so two
/// sweepers race to different departures rather than the same one. No
/// lease, because there is nothing to serialize: the work is already
/// partitioned by whoever wins the row.
pub async fn run(state: AppState, every: std::time::Duration) {
    loop {
        tokio::time::sleep(every).await;
        // Drain rather than take one: a rack losing several nodes at
        // once should not wait a full interval per departure.
        loop {
            match sweep_once(&state).await {
                Ok(swept) if swept.node_id.is_empty() => break,
                Ok(swept) => {
                    state.directory_gauges.count(&state.directory_gauges.sweeps);
                    state
                        .directory_gauges
                        .add(&state.directory_gauges.swept_rows, swept.rows);
                }
                Err(error) => {
                    actias_common::tracing::warn!(
                        %error,
                        "a crash sweep failed; the next pass retries"
                    );
                    break;
                }
            }
        }
    }
}
