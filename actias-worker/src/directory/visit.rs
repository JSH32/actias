//! The verified read: a listing whose every candidate is checked
//! against its object's own shipping manifest before it is served.
//!
//! No restores. The index found the candidate; the object's manifest
//! carries its settled row; the two versions decide everything. Equal
//! means the index IS the settled truth and the sql predicate already
//! evaluated it. Newer means the fresh row is rechecked in memory and
//! served instead. Anything unprovable is served flagged rather than
//! dropped, because the one failure the directory refuses is the row
//! that silently vanishes. The whole ladder lives in
//! [`actias_worker_core::directory::verify`]; this module only fetches
//! manifests and keeps the page's order.
//!
//! `limit` bounds candidates examined, not entries returned: a page may
//! come back short with a cursor, and that is the honest shape, since
//! an exact count against a stale superset would take unbounded work.

use actias_worker_core::directory::overlay::Query;
use actias_worker_core::directory::scratch::{Scratched, evaluate_scratch};
use actias_worker_core::directory::verify::{
    Settled, Verdict, Visited, VisitedPage, against_manifest, matches,
};
use actias_worker_core::directory::version::RowVersion;
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::runtime::ActiasRuntime;
use futures::StreamExt;

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// Manifest fetches in flight at once. Bounded so one wide page cannot
/// point-load the store; ordered, so the page keeps the listing's
/// order.
const FETCH_WIDTH: usize = 8;

/// Objects one page may restore for its tail.
///
/// A failed derivation is class-wide, not scattered: a throwing derive,
/// a blown budget or a bad publish fails every object of the class on
/// every write. So a visit over a broken class can name a whole page of
/// recompute candidates, and without a cap one query would restore 500
/// objects. Past this the rest come back flagged, which is the honest
/// answer and the one the ladder already gives for anything it cannot
/// check.
const TAIL_LIMIT: usize = 16;

/// Recomputed rows a node remembers, keyed by the version they were
/// derived at.
///
/// A restore is the most expensive thing this whole feature does, and
/// the tail's trigger persists: a row stays failed until its object
/// writes successfully again. Without this, every visit over a broken
/// class pays the same storm, so a console left refreshing turns one
/// user's bug into a steady load on the object store.
///
/// The key is the row's own version, which is what makes invalidation
/// free: a later write mints a new rev, so its recomputation simply
/// misses. Nothing is ever stale here, only absent.
#[derive(Default)]
pub struct Recomputed {
    rows: std::sync::Mutex<
        std::collections::HashMap<
            (String, i64, i64, u64),
            actias_worker_core::directory::evaluate::Row,
        >,
    >,
}

/// Entries kept before the cache forgets the oldest. Rows are small
/// (a 4KB cap each) and only broken classes populate this at all.
const CACHE_MAX: usize = 4096;

impl Recomputed {
    fn get(
        &self,
        key: &(String, i64, i64, u64),
    ) -> Option<actias_worker_core::directory::evaluate::Row> {
        self.rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
    }

    fn put(&self, key: (String, i64, i64, u64), row: actias_worker_core::directory::evaluate::Row) {
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A flat cap rather than an lru: entries are only written for
        // classes in a failed state, so the map is small or the node
        // has a much larger problem than cache policy.
        if rows.len() >= CACHE_MAX {
            rows.clear();
        }
        rows.insert(key, row);
    }
}

/// Answers one verified listing.
///
/// # Errors
/// The listing's own refusals (unknown field, building field), or the
/// store's message. A single candidate's failed fetch never fails the
/// page; that candidate comes back flagged.
pub async fn visit(
    state: &AppState,
    class: &ClassKey,
    query: Query,
) -> Result<VisitedPage, String> {
    // The tree is needed twice: once translated into sql by the
    // listing, once by the in-memory recheck. Cloned here because the
    // query moves into the overlay call.
    let where_ = query.where_.clone();
    let page = crate::directory::read::candidates(state, class, query).await?;
    let cursor = page.cursor;

    // Ordered concurrency: fetches overlap, the page's order holds.
    let verdicts: Vec<Verdict> =
        futures::stream::iter(page.candidates.into_iter().map(|candidate| {
            let state = state.clone();
            let where_ = where_.clone();
            async move {
                let said = fetch_settled(&state, &candidate.entry.object_id).await;
                against_manifest(candidate, &said, &where_)
            }
        }))
        .buffered(FETCH_WIDTH)
        .collect()
        .await;

    // The tail: candidates whose newest derivation failed, so neither
    // the index nor the manifest holds the current row. Only a restored
    // copy does. Built lazily, because a page with none of these must
    // not pay for a vm.
    let mut recompute: Vec<(actias_worker_core::directory::overlay::Entry, RowVersion)> =
        Vec::new();
    let mut entries = Vec::with_capacity(verdicts.len());
    let gauges = &state.directory_gauges;
    for verdict in verdicts {
        match verdict {
            Verdict::Verified(entry) => {
                gauges.count(&gauges.visit_verified);
                entries.push(Visited {
                    entry,
                    unverified: false,
                    reason: None,
                });
            }
            Verdict::Dropped => gauges.count(&gauges.visit_dropped),
            Verdict::Flagged { entry, reason } => {
                gauges.count(&gauges.visit_flagged);
                entries.push(Visited {
                    entry,
                    unverified: true,
                    reason: Some(reason),
                });
            }
            Verdict::Recompute { entry, failed_at } => {
                gauges.count(&gauges.visit_recomputed);
                recompute.push((entry, failed_at));
            }
        }
    }

    if !recompute.is_empty() {
        entries.extend(recomputed(state, class, &where_, recompute).await);
    }

    Ok(VisitedPage { entries, cursor })
}

/// What the object's manifest says, as the ladder consumes it. A fetch
/// error maps to [`Settled::Missing`]'s flagged path via its own
/// message: the candidate is kept either way, which is the superset
/// principle doing its job.
async fn fetch_settled(state: &AppState, object_id: &str) -> Settled {
    match state.object_store.manifest(object_id).await {
        Ok(None) => Settled::Missing,
        Ok(Some(manifest)) if manifest.deleted => Settled::Deleted,
        Ok(Some(manifest)) => match manifest.directory {
            None => Settled::NoRow,
            Some(row) => Settled::Row {
                version: RowVersion {
                    epoch: manifest.epoch,
                    rev: row.rev.max(0) as u64,
                    dver: row.dver.max(0) as u64,
                },
                pairs: row.fields,
                failed: row.failed,
            },
        },
        Err(error) => {
            actias_common::tracing::warn!(
                %object_id,
                %error,
                "a manifest could not be read during a visit; the candidate is kept flagged"
            );
            Settled::Missing
        }
    }
}

/// Recomputes the tail from restored copies.
///
/// One runtime for the whole tail, reused across objects the way the
/// backfill reuses one: a vm per candidate would dwarf the restores it
/// is there to serve.
///
/// A failure here keeps the candidate flagged rather than dropping it,
/// which is the same rule the rest of the ladder obeys: not knowing is
/// never grounds for a false negative.
async fn recomputed(
    state: &AppState,
    class: &ClassKey,
    where_: &actias_worker_core::directory::predicate::Where,
    candidates: Vec<(actias_worker_core::directory::overlay::Entry, RowVersion)>,
) -> Vec<Visited> {
    let flagged = |entry, reason: String| Visited {
        entry,
        unverified: true,
        reason: Some(reason),
    };

    // Serve whatever a previous visit already recomputed, before paying
    // for a vm at all: a broken class is queried as often as a healthy
    // one, and the restores are what cost.
    let mut served = Vec::new();
    let mut cold = Vec::new();
    for (entry, failed_at) in candidates {
        let key = (
            entry.object_id.clone(),
            failed_at.rev as i64,
            failed_at.dver as i64,
            failed_at.epoch,
        );
        match state.directory_recomputed.get(&key) {
            Some(row) => match matches(where_, &row) {
                Ok(true) => served.push(Visited {
                    entry: actias_worker_core::directory::overlay::Entry {
                        name: entry.name,
                        object_id: entry.object_id,
                        fields: row,
                    },
                    unverified: false,
                    reason: None,
                }),
                Ok(false) => {}
                Err(reason) => served.push(flagged(entry, reason)),
            },
            None => cold.push((entry, key)),
        }
    }

    // Past the cap the rest are flagged rather than restored. One query
    // must not be able to restore a whole page: the trigger is
    // class-wide, so a broken class names every object it has.
    if cold.len() > TAIL_LIMIT {
        let overflow = cold.split_off(TAIL_LIMIT);
        served.extend(overflow.into_iter().map(|(entry, _)| {
            flagged(
                entry,
                format!(
                    "more than {TAIL_LIMIT} rows on this page need recomputing;                      the rest are checked as the page is walked again"
                ),
            )
        }));
    }

    if cold.is_empty() {
        return served;
    }
    let candidates = cold;

    let key = ObjectKey::received(&class.scope_id, &class.class, &candidates[0].0.name);
    let revision = match crate::routing::owner_prepared(state, &key).await {
        Ok(revision) => revision,
        Err(error) => {
            served.extend(
                candidates
                    .into_iter()
                    .map(|(entry, _)| flagged(entry, error.clone())),
            );
            return served;
        }
    };
    let runtime = match ActiasRuntime::new(
        revision.clone(),
        state.clients.kv.clone(),
        state.egress.clone(),
        None,
        state.secret_client.clone(),
        Some(state.guest_limits.wall_secs),
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = error.to_string();
            served.extend(
                candidates
                    .into_iter()
                    .map(|(entry, _)| flagged(entry, error.clone())),
            );
            return served;
        }
    };
    state.guest_limits.apply(&runtime);

    for (entry, cache_key) in candidates {
        let file = state
            .object_data_dir
            .join(format!("directory-visit-{}.db", entry.object_id));
        let _ = std::fs::remove_file(&file);

        let restored = state.object_store.restore(&entry.object_id, &file).await;
        let row = match restored {
            // Gone from the store: a destroyed object matches nothing,
            // so dropping it invents no false negative.
            Ok(false) => {
                let _ = std::fs::remove_file(&file);
                continue;
            }
            Err(error) => Err(error),
            Ok(true) => match actias_worker_core::storage::SqliteStorage::open(&file) {
                Err(error) => Err(error),
                Ok(storage) => evaluate_scratch(
                    &runtime,
                    revision.clone(),
                    storage,
                    &class.class,
                    &entry.name,
                    actias_worker_core::directory::DEFAULT_EVAL_BUDGET_MS,
                )
                .map(|scratched| match scratched {
                    Scratched::Row(row) => row,
                    Scratched::NoDirectory => Vec::new(),
                }),
            },
        };
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(file.with_extension("db-wal"));
        let _ = std::fs::remove_file(file.with_extension("db-shm"));

        match row {
            Err(error) => served.push(flagged(entry, error)),
            Ok(row) => {
                // Remembered under the version that failed, so a later
                // write simply misses rather than reading stale.
                state.directory_recomputed.put(cache_key, row.clone());
                match matches(where_, &row) {
                    // The recomputed row is what the caller gets, not the
                    // last good one it was found by.
                    Ok(true) => served.push(Visited {
                        entry: actias_worker_core::directory::overlay::Entry {
                            name: entry.name,
                            object_id: entry.object_id,
                            fields: row,
                        },
                        unverified: false,
                        reason: None,
                    }),
                    Ok(false) => {}
                    Err(reason) => served.push(flagged(entry, reason)),
                }
            }
        }
    }
    served
}
