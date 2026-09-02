//! Building a newly declared field into rows that predate it.
//!
//! A publish that changes a class's field set mints a new declaration
//! version, and every row derived under an older one may simply lack
//! the new field. A query on it would then miss those objects silently,
//! which is the one failure this design refuses, so such a field is
//! refused until every row carries it. This is what makes that stop
//! being true.
//!
//! Without it, adding a field is a one-way door: the field is queryable
//! only after every object happens to write again, and an object that
//! never writes again never carries it.
//!
//! Nothing is woken. Each laggard is restored to a throwaway copy,
//! evaluated read-only, and discarded, which is why this can run for a
//! class with zero residencies. The cost is one restore per object, so
//! it runs once per object per schema change and never on a write path.
//!
//! The floor lifts by itself. `min_dver` is the lowest dver across live
//! rows, computed by the compactor from the rows it merges, so once the
//! laggards carry the new version the field simply stops being
//! building. Nothing here writes a manifest.

use std::sync::Arc;

use actias_worker_core::directory::delta::{self, DeltaRow};
use actias_worker_core::directory::row::{Pair, RowSnapshot};
use actias_worker_core::directory::scratch::{Scratched, evaluate_scratch};
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::runtime::ActiasRuntime;

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// Objects one pass may rebuild. A bound rather than a batch size: the
/// pass is resumable because the worklist is "rows below the target",
/// so the next pass simply picks up whatever is left.
const PER_PASS: usize = 256;

/// What one backfill pass did.
#[derive(Debug, Default, PartialEq)]
pub struct Backfilled {
    /// Rows re-derived and offered at the class's current version.
    pub rebuilt: usize,
    /// Objects whose settled state could not be restored, or whose
    /// derive failed. Left behind for the next pass rather than
    /// dropped: a row that cannot be re-derived keeps its old one,
    /// which is stale rather than missing.
    pub skipped: usize,
    /// Rows still behind after this pass, so an operator can see
    /// progress rather than guess at it.
    pub remaining: usize,
}

/// Re-derives one class's laggard rows at its current declaration.
///
/// # Errors
/// Returns the store's message when the class cannot be read at all. A
/// single object's failure is counted, never fatal: a pass that rebuilds
/// most of a class is worth more than one that rebuilds none.
pub async fn backfill_class(state: &AppState, class: &ClassKey) -> Result<Backfilled, String> {
    let recorded = state
        .object_store
        .directory_manifest(&class.scope_id, &class.class)
        .await?
        .unwrap_or_default();
    // What is building is judged against the owner's current
    // declaration, not only the recorded set: a field published
    // against a quiet class has reached no fold yet (no write since,
    // so no delta carried it), and the delta this pass writes is what
    // carries it there. The rows it re-derives are stamped at the
    // declaration's version, so the fold that records the field set
    // lifts the floor for them in the same step.
    let manifest = crate::directory::read::declared_manifest(state, class, &recorded, true).await;

    // Nothing is waiting on a backfill: the overwhelmingly common case,
    // and it costs one manifest read.
    if manifest.building().is_empty() {
        return Ok(Backfilled::default());
    }

    let target = manifest.dver;
    let behind = crate::directory::read::rows_behind(state, class, target, PER_PASS).await?;
    if behind.is_empty() {
        return Ok(Backfilled::default());
    }

    // One runtime for the whole pass. Building a vm per object would
    // dwarf the restores, and `evaluate_scratch` swaps the home per
    // call precisely so this can be reused.
    let key = ObjectKey::received(&class.scope_id, &class.class, &behind[0].name);
    let revision = crate::routing::owner_prepared(state, &key).await?;
    let runtime = ActiasRuntime::new(
        revision.clone(),
        state.clients.kv.clone(),
        state.egress.clone(),
        None,
        state.secret_client.clone(),
        Some(state.guest_limits.wall_secs),
    )
    .await
    .map_err(|error| error.to_string())?;
    state.guest_limits.apply(&runtime);

    // The declaration the rebuilt rows are derived under. Absent means
    // the owner's contract predates declared fields, and re-deriving
    // against it would stamp version zero and undo the pass.
    let Some(declaration) = revision.directory_spec(&class.class) else {
        return Err(format!(
            "'{}' has building fields but its owner's contract declares none; republish it",
            class.class
        ));
    };

    let mut summary = Backfilled::default();
    let mut rows = Vec::with_capacity(behind.len());
    for object in &behind {
        match rebuild_one(state, &runtime, revision.clone(), class, object, target).await {
            Ok(Some(row)) => {
                summary.rebuilt += 1;
                rows.push(row);
            }
            // No directory, or a destroyed object: nothing to rebuild
            // and nothing wrong.
            Ok(None) => {}
            Err(error) => {
                summary.skipped += 1;
                actias_common::tracing::warn!(
                    class = %class.class,
                    name = %object.name,
                    %error,
                    "an object could not be rebuilt; its row stays stale and the next pass retries"
                );
            }
        }
    }

    if rows.is_empty() {
        return Ok(summary);
    }

    rows.sort_by(|left, right| left.object_id.cmp(&right.object_id));
    let bytes = delta::encode(&rows, Some(&declaration), &state.object_data_dir)?;
    let name = blake3::hash(&bytes).to_hex().to_string();
    state
        .object_store
        .put_directory_delta(&class.scope_id, &class.class, &name, bytes)
        .await?;

    summary.remaining = behind.len().saturating_sub(summary.rebuilt);
    actias_common::tracing::info!(
        class = %class.class,
        dver = target,
        rebuilt = summary.rebuilt,
        skipped = summary.skipped,
        "a directory backfill rebuilt rows at the class's current declaration"
    );
    Ok(summary)
}

/// Restores one object, derives its row, and discards the copy.
async fn rebuild_one(
    state: &AppState,
    runtime: &ActiasRuntime,
    revision: Arc<actias_worker_core::runtime::PreparedRevision>,
    class: &ClassKey,
    object: &actias_worker_core::directory::repair::Indexed,
    dver: u64,
) -> Result<Option<DeltaRow>, String> {
    // Named per object so two passes cannot collide on one file, and
    // removed however this returns.
    let file = state
        .object_data_dir
        .join(format!("directory-backfill-{}.db", object.object_id));
    let _ = std::fs::remove_file(&file);

    let restored = state.object_store.restore(&object.object_id, &file).await;
    let outcome = match restored {
        // Gone from the store, so there is nothing to re-derive. The
        // reconciliation pass is what retires the row.
        Ok(false) => Ok(None),
        Err(error) => Err(error),
        Ok(true) => {
            let storage = actias_worker_core::storage::SqliteStorage::open(&file)
                .map_err(|error| format!("the restored copy could not be opened: {error}"))?;

            // The rev comes from the object's own settled row, not from
            // this pass: a backfilled row describes the same state the
            // last write settled, so inventing a rev would order it
            // ahead of a real write that has not shipped yet.
            let settled = state.object_store.manifest(&object.object_id).await?;
            let rev = settled
                .as_ref()
                .and_then(|manifest| manifest.directory.as_ref())
                .map_or(0, |row| row.rev);

            match evaluate_scratch(
                runtime,
                revision,
                storage,
                &class.class,
                &object.name,
                actias_worker_core::directory::DEFAULT_EVAL_BUDGET_MS,
            ) {
                Err(error) => Err(error),
                Ok(Scratched::NoDirectory) => Ok(None),
                Ok(Scratched::Row(row)) => Ok(Some(DeltaRow {
                    object_id: object.object_id.clone(),
                    name: object.name.clone(),
                    epoch: object.epoch,
                    snapshot: RowSnapshot {
                        rev,
                        dver: dver as i64,
                        fields: row
                            .iter()
                            .map(|(name, value)| {
                                let (kind, text) =
                                    actias_worker_core::directory::row::encode_pair(value);
                                Pair {
                                    field: name.clone(),
                                    kind,
                                    value: text,
                                }
                            })
                            .collect(),
                        failed: None,
                    },
                    tombstone: false,
                })),
            }
        }
    };

    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(file.with_extension("db-wal"));
    let _ = std::fs::remove_file(file.with_extension("db-shm"));
    outcome
}
