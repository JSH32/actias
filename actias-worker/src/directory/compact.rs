//! The compactor: deltas become a base, and the class's field set is
//! discovered on the way.
//!
//! This is the one serialized point in the directory, and it is
//! deliberately off the write path: nodes never coordinate to write a
//! delta, they coordinate only to fold them. A lease decides who
//! compacts, and the manifest's generation fences whoever publishes
//! second, so a lapsed lease costs an orphaned base rather than a
//! corrupted class.
//!
//! It is also where a published field set reaches the manifest. Each
//! delta carries the declaration its rows were derived under, the
//! merge collects them, and `Manifest::observe_declaration` folds
//! them in, bumping the field-set generation only when the set
//! actually changed. An ordinary deploy that changes no field
//! therefore rebuilds nothing.

use actias_worker_core::directory::{compact, manifest::Manifest};

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// Unfolded deltas that trigger a fold on their own; below this the
/// class waits for [`FOLD_AFTER`]. Readers apply that many in place at
/// worst, which is cheap next to a base rewrite.
const FOLD_AT_DELTAS: usize = 16;

/// The longest a delta waits to be folded. Bounds how many deltas a
/// cold reader materializes, and how long the store keeps a delta
/// beside a base that has not absorbed it.
const FOLD_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// When each class first had an unfolded delta this node noticed,
/// node-local: the lease already makes whichever node folds first
/// correct, and a timer per node only ever makes a fold later, never
/// wrong.
fn pending_since()
-> &'static std::sync::Mutex<std::collections::HashMap<ClassKey, std::time::Instant>> {
    static PENDING: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<ClassKey, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The lease a compactor holds. Derived from the class rather than any
/// object, so it reuses the placement store's existing claim path with
/// no schema of its own; the empty identity fields keep it a bare
/// lease, with no directory row of its own.
pub(crate) fn lease_id(class: &ClassKey) -> String {
    blake3::hash(format!("directory-compact:{}:{}", class.scope_id, class.class).as_bytes())
        .to_hex()
        .to_string()
}

/// Folds every unfolded delta for one class into a new base.
///
/// Returns whether anything was written; a class with nothing pending
/// is the common case and costs one list.
///
/// # Errors
/// Returns the store's or the registry's message. Losing the lease or
/// the generation fence is an error like any other: the work is simply
/// redone by whoever holds the lease next.
/// `now` folds whatever is fresh regardless of the cadence below: the
/// reconciliation pass folds the delta it just offered so the repair
/// lands in the same pass, and an operator's rebuild does the same.
pub async fn compact_class(state: &AppState, class: &ClassKey, now: bool) -> Result<bool, String> {
    let deltas = state
        .object_store
        .directory_deltas(&class.scope_id, &class.class)
        .await?;
    let manifest = state
        .object_store
        .directory_manifest(&class.scope_id, &class.class)
        .await?
        .unwrap_or_default();

    let folded: std::collections::HashSet<&str> =
        manifest.folded.iter().map(String::as_str).collect();
    let fresh: Vec<String> = deltas
        .iter()
        .filter(|name| !folded.contains(name.as_str()))
        .cloned()
        .collect();
    // Nothing fresh is the common case and costs one list. The one
    // exception is a manifest from before the identity checksum: with
    // no fold there is nothing to rewrite it, so reconciliation would
    // read "not known", rebuild, and find the same rows again on every
    // pass forever. Folding an empty set re-lays the same
    // content-addressed base and records the checksum, once.
    if fresh.is_empty() && manifest.identities.is_some() {
        pending_since()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(class);
        return Ok(false);
    }
    // A fold rewrites the whole base (measured: ~1KB per row shipped to
    // the store, so a 100k-row class is a 100MB PUT), and readers apply
    // unfolded deltas in place, so folding every interval buys nothing
    // but write amplification. Fold when enough deltas have gathered,
    // or when the oldest has waited long enough that a reader's
    // materialization would start to cost, whichever comes first. A
    // manifest with no checksum yet folds at once: that rewrite is the
    // point of it.
    if !now && manifest.identities.is_some() && !fresh.is_empty() {
        let mut since = pending_since().lock().unwrap_or_else(|p| p.into_inner());
        let first = *since
            .entry(class.clone())
            .or_insert_with(std::time::Instant::now);
        if fresh.len() < FOLD_AT_DELTAS && first.elapsed() < FOLD_AFTER {
            return Ok(false);
        }
        since.remove(class);
    }

    let node_id = state
        .node_identity
        .read()
        .expect("no poisoned lock")
        .clone()
        .ok_or_else(|| "this node has not finished registering".to_owned())?;
    let lease = state
        .registry
        .clone()
        .acquire_lease(
            actias_worker_core::proto::node_registry::AcquireLeaseRequest {
                object_id: lease_id(class),
                node_id,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?
        .into_inner();
    if !lease.acquired {
        // Someone else is folding this class; their base is as good as
        // ours would have been.
        return Ok(false);
    }

    // Everything the merge reads is immutable and content-addressed, so
    // a delta written while this runs is simply not in `fresh` and waits
    // for the next pass.
    let base = match &manifest.base {
        Some(name) => Some(
            state
                .object_store
                .directory_file(&class.scope_id, &class.class, "bases", name)
                .await?,
        ),
        None => None,
    };
    let mut bytes = Vec::with_capacity(fresh.len());
    for name in &fresh {
        bytes.push(
            state
                .object_store
                .directory_file(&class.scope_id, &class.class, "deltas", name)
                .await?,
        );
    }

    let scratch = state.object_data_dir.clone();
    let previous_base = manifest.base.clone();
    let merged = {
        let base = base.clone();
        let manifest = manifest.clone();
        tokio::task::spawn_blocking(move || {
            compact::merge(base.as_deref(), &bytes, &manifest, &scratch)
        })
        .await
        .map_err(|e| e.to_string())??
    };

    let name = format!("b-{}", blake3::hash(&merged.bytes).to_hex());
    state
        .object_store
        .put_directory_base(&class.scope_id, &class.class, &name, merged.bytes)
        .await?;

    let mut next = Manifest {
        generation: manifest.generation + 1,
        base: Some(name),
        folded: deltas,
        min_dver: merged.min_dver,
        rows: merged.rows,
        identities: Some(merged.identities),
        ..manifest
    };
    // The field sets the folded deltas declared. Publish already
    // decided which version each is, so this only records them; the
    // merge converges whatever order the deltas arrived in, because
    // deltas are an unordered bag and one can always be late.
    for declaration in &merged.declarations {
        if next.observe_declaration(declaration) {
            actias_common::tracing::info!(
                class = %class.class,
                dver = next.dver,
                "the directory recorded a published field set; new fields build as rows are re-derived"
            );
        }
    }

    state
        .object_store
        .put_directory_manifest(&class.scope_id, &class.class, &next)
        .await?;
    state.directory_gauges.count(&state.directory_gauges.folds);

    // Only after the manifest names the new base: a crash between the
    // two leaves an orphaned base, which the next pass collects, rather
    // than a manifest naming bytes that are gone.
    if let Err(error) = state
        .object_store
        .collect_directory_garbage(
            &class.scope_id,
            &class.class,
            &next,
            previous_base.as_deref(),
        )
        .await
    {
        actias_common::tracing::warn!(
            class = %class.class,
            %error,
            "directory garbage was left behind; the next pass retries"
        );
    }

    Ok(true)
}

/// Compacts the classes this node has written rows for, forever.
///
/// Node-scoped discovery on purpose: a node that never wrote a row for
/// a class has no reason to fold it, and some node that did will. The
/// lease makes overlap harmless.
pub async fn run(state: AppState, every: std::time::Duration) {
    loop {
        tokio::time::sleep(every).await;
        for class in state.directory_sync.known_classes() {
            if let Err(error) = compact_class(&state, &class, false).await {
                state
                    .directory_gauges
                    .count(&state.directory_gauges.fold_failures);
                actias_common::tracing::warn!(
                    class = %class.class,
                    %error,
                    "directory compaction failed; retrying on the next pass"
                );
            }
        }
    }
}
