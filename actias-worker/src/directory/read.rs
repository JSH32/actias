//! Answering a directory listing on this node.
//!
//! The read path takes no lease, wakes no object, and talks to no other
//! node: it reads the class's manifest, materializes an overlay of the
//! base and whatever deltas have not been folded, and runs the query
//! against that. Bases and deltas are content-addressed and immutable,
//! so a warm node answers from local disk, and the manifest is the one
//! thing worth re-reading.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use actias_worker_core::directory::manifest::Manifest;
use actias_worker_core::directory::overlay::{Overlay, Page, Query};

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// Rows one page may carry, whatever a caller asks for. A listing
/// chooses which objects to call, so a page past this is a caller that
/// wanted the whole class; the cursor is the way to have it.
pub const MAX_LIMIT: i64 = 500;

/// Overlays this node has materialized, by class. Keyed by generation:
/// bases are immutable, so an overlay stays valid until a compaction
/// publishes a new one, and rebuilding is the only invalidation.
#[derive(Default)]
pub struct Overlays {
    built: Mutex<HashMap<ClassKey, Arc<Built>>>,
}

pub struct Built {
    pub overlay: Overlay,
    pub manifest: Manifest,
    /// Deltas folded into this overlay beyond the manifest's base, so a
    /// later query can tell whether anything new has arrived.
    pub deltas: Vec<String>,
    /// When this overlay last answered anything, for eviction. An
    /// overlay is pure cache, rebuildable from immutable files, so an
    /// idle one is disk a node is holding for nothing.
    pub touched: std::sync::Mutex<std::time::Instant>,
}

impl Built {
    fn touch(&self) {
        *self
            .touched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = std::time::Instant::now();
    }

    fn idle_for(&self) -> std::time::Duration {
        self.touched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
    }
}

impl Overlays {
    fn current(&self, class: &ClassKey) -> Option<Arc<Built>> {
        self.built
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(class)
            .cloned()
    }

    fn store(&self, class: ClassKey, built: Arc<Built>) {
        self.built
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(class, built);
    }

    /// Forgets overlays idle past `ttl`, answering what it dropped.
    ///
    /// Only the map entry goes here; the file is removed by the caller,
    /// which knows the data directory. Dropping is always safe: an
    /// overlay is rebuilt from the base and deltas, which are immutable
    /// and content-addressed, so eviction costs a rebuild and never
    /// correctness.
    fn evict_idle(&self, ttl: std::time::Duration) -> Vec<ClassKey> {
        let mut built = self
            .built
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cold: Vec<ClassKey> = built
            .iter()
            .filter(|(_, entry)| entry.idle_for() > ttl)
            .map(|(class, _)| class.clone())
            .collect();
        for class in &cold {
            built.remove(class);
        }
        cold
    }
}

/// Drops idle overlays and their files, forever.
///
/// Without this a node accretes one overlay file per class it has ever
/// been asked about and never gives one back, which is the one place
/// the directory's disk grows with the cluster's class count rather
/// than with what this node serves.
pub async fn evict(state: AppState, every: std::time::Duration, ttl: std::time::Duration) {
    loop {
        tokio::time::sleep(every).await;
        for class in state.directory_overlays.evict_idle(ttl) {
            let path = state.object_data_dir.join(format!(
                "directory-{}-{}.overlay",
                class.scope_id, class.class
            ));
            let _ = std::fs::remove_file(&path);
            actias_common::tracing::debug!(
                class = %class.class,
                "an idle directory overlay left the disk; the next query rebuilds it"
            );
        }
    }
}

/// Materializes the class if this node has nothing current, and answers
/// the query.
///
/// # Errors
/// Returns the store's message, or the query's own refusal (an unknown
/// field, a field still building).
pub async fn list(state: &AppState, class: &ClassKey, query: Query) -> Result<Page, String> {
    let built = ensure(state, class).await?;
    let manifest = declared_view(state, class, &built).await;
    built
        .overlay
        .list(&query, &manifest)
        .inspect_err(|_| state.directory_gauges.refused(&class.class))
}

/// The manifest as the owner's current declaration says it reads.
///
/// The recorded manifest learns a field set only when a delta carrying
/// it is folded, which is some object's next write plus a compaction
/// after the publish. Without this view, a query naming a field the
/// author just published would be refused as "not a directory field",
/// which reads as a typo to the author who typed it a second ago.
/// Folding the declaration into a query-time view closes that gap: the
/// new field reads as building, which is exactly true of it, and a
/// field the declaration dropped reads as unknown from the moment it
/// was dropped.
///
/// Nothing is persisted here; the compactor still records the set, and
/// the recorded one is what the view starts from, so a stale owner
/// cache (a pointer ttl) can only ever be late, never wrong about a
/// field that rows already carry. A class with no rows at all has
/// nothing behind, so its floor is the declaration's own version,
/// which is what the compactor would record for it.
async fn declared_view(state: &AppState, class: &ClassKey, built: &Built) -> Manifest {
    let has_rows = built.manifest.rows > 0 || !built.deltas.is_empty();
    declared_manifest(state, class, &built.manifest, has_rows).await
}

/// [`declared_view`] over a manifest the caller already holds, for
/// the loops that read the store's manifest rather than an overlay:
/// the backfill decides what is building from this, and the
/// reconciliation pass decides whether a backfill is due from this,
/// so a field published against a quiet class (no write since, so no
/// delta has carried the declaration to a fold yet) still gets built
/// on the next pass instead of waiting for an object to write.
pub(crate) async fn declared_manifest(
    state: &AppState,
    class: &ClassKey,
    recorded: &Manifest,
    has_rows: bool,
) -> Manifest {
    let mut view = recorded.clone();
    // The owner is the class's, not any object's: the name is empty
    // because the contract lookup keys on the class, and the instance
    // directory fallback (which does key on a name) cannot apply to a
    // class whose contract declares a directory.
    let key = actias_worker_core::identity::ObjectKey::received(&class.scope_id, &class.class, "");
    let spec = match crate::routing::owner_prepared(state, &key).await {
        Ok(revision) => revision.directory_spec(&class.class),
        Err(error) => {
            actias_common::tracing::debug!(
                class = %class.class,
                %error,
                "the owner's declaration is not readable; the recorded field set answers"
            );
            None
        }
    };
    if let Some(spec) = spec
        && view.observe_declaration(&spec)
        && !has_rows
    {
        view.min_dver = view.dver;
    }
    view
}

/// Fields the class has seen but not yet finished backfilling. A
/// query naming one is refused; this reports the rest so a console can
/// show progress rather than leaving a column mysteriously missing.
///
/// Best effort: a store that cannot answer reports nothing building,
/// because this is decoration on a page that already succeeded.
pub async fn building(state: &AppState, class: &ClassKey) -> Vec<String> {
    // The overlay this page was just answered from, when it is still
    // here; otherwise the store. Either way the owner's declaration is
    // folded in, so a field published a second ago shows as building
    // rather than not at all.
    let built = match state.directory_overlays.current(class) {
        Some(built) => built,
        None => match ensure(state, class).await {
            Ok(built) => built,
            Err(_) => return Vec::new(),
        },
    };
    declared_view(state, class, &built)
        .await
        .building()
        .into_iter()
        .map(|field| field.name.clone())
        .collect()
}

/// One page of candidates with their indexed versions, for the
/// verified read. Same overlay, same refusals as [`list`].
///
/// # Errors
/// Same as [`list`].
pub async fn candidates(
    state: &AppState,
    class: &ClassKey,
    query: Query,
) -> Result<actias_worker_core::directory::overlay::CandidatePage, String> {
    let built = ensure(state, class).await?;
    let manifest = declared_view(state, class, &built).await;
    let built = built.clone();
    tokio::task::spawn_blocking(move || built.overlay.candidates(&query, &manifest))
        .await
        .map_err(|e| e.to_string())?
        .inspect_err(|_| state.directory_gauges.refused(&class.class))
}

/// Rows derived under a declaration older than `dver`, oldest first.
/// The backfill's worklist, over the same materialized overlay a
/// listing reads.
///
/// # Errors
/// Returns the store's message.
pub async fn rows_behind(
    state: &AppState,
    class: &ClassKey,
    dver: u64,
    limit: usize,
) -> Result<Vec<actias_worker_core::directory::repair::Indexed>, String> {
    let built = ensure(state, class).await?;
    let built = built.clone();
    tokio::task::spawn_blocking(move || built.overlay.behind(dver, limit))
        .await
        .map_err(|e| e.to_string())?
}

/// Every live row's identity and held epoch, for reconciling the index
/// against the objects that still exist.
///
/// Reuses the same materialized overlay a listing reads, so a rebuild
/// pays nothing extra on a warm node.
///
/// # Errors
/// Returns the store's message.
pub async fn indexed_identities(
    state: &AppState,
    class: &ClassKey,
) -> Result<Vec<actias_worker_core::directory::repair::Indexed>, String> {
    let built = ensure(state, class).await?;
    let built = built.clone();
    tokio::task::spawn_blocking(move || built.overlay.identities())
        .await
        .map_err(|e| e.to_string())?
}

/// The overlay for a class, rebuilt when the manifest moved on or a
/// delta arrived that the current one has not absorbed.
async fn ensure(state: &AppState, class: &ClassKey) -> Result<Arc<Built>, String> {
    let manifest = state
        .object_store
        .directory_manifest(&class.scope_id, &class.class)
        .await?
        .unwrap_or_default();
    let deltas = state
        .object_store
        .directory_deltas(&class.scope_id, &class.class)
        .await?;
    let unfolded: Vec<String> = deltas
        .iter()
        .filter(|name| !manifest.folded.contains(name))
        .cloned()
        .collect();

    // Immutability is what makes this check enough: same generation and
    // same unfolded set means the same bytes, so the overlay already
    // holds them.
    if let Some(current) = state.directory_overlays.current(class)
        && current.overlay.generation == manifest.generation
    {
        if current.deltas == unfolded {
            current.touch();
            return Ok(current);
        }
        // Same generation, more deltas: apply only the new ones in
        // place. A hot class flushes a delta every interval, and
        // rebuilding the whole overlay for each one is O(rows) per
        // flush per node, which at 100k rows is seconds per flush.
        // The unfolded list only ever grows within a generation (the
        // compactor folds by publishing a new generation), so anything
        // not in the built list is new.
        let known: std::collections::HashSet<&str> =
            current.deltas.iter().map(String::as_str).collect();
        let fresh: Vec<String> = unfolded
            .iter()
            .filter(|name| !known.contains(name.as_str()))
            .cloned()
            .collect();
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
        let applied = {
            let current = current.clone();
            tokio::task::spawn_blocking(move || current.overlay.apply(&bytes, &scratch))
                .await
                .map_err(|e| e.to_string())?
        };
        if applied.is_ok() {
            state
                .directory_gauges
                .count(&state.directory_gauges.overlay_applies);
            // The same file, now carrying the new deltas; a fresh entry
            // records what it holds. The manifest is the same
            // generation, so the declared view stays valid.
            let built = Arc::new(Built {
                overlay: Overlay::reopen(&current.overlay),
                manifest: manifest.clone(),
                deltas: unfolded,
                touched: std::sync::Mutex::new(std::time::Instant::now()),
            });
            state.directory_overlays.store(class.clone(), built.clone());
            return Ok(built);
        }
        // An apply that failed falls through to a full rebuild, which
        // is the cost this path exists to avoid.
    }

    let base = match &manifest.base {
        Some(name) => Some(
            state
                .object_store
                .directory_file(&class.scope_id, &class.class, "bases", name)
                .await?,
        ),
        None => None,
    };
    let mut bytes = Vec::with_capacity(unfolded.len());
    for name in &unfolded {
        bytes.push(
            state
                .object_store
                .directory_file(&class.scope_id, &class.class, "deltas", name)
                .await?,
        );
    }

    let path = state.object_data_dir.join(format!(
        "directory-{}-{}.overlay",
        class.scope_id, class.class
    ));
    let scratch = state.object_data_dir.clone();
    let started = std::time::Instant::now();
    let for_build = manifest.clone();
    let overlay = tokio::task::spawn_blocking(move || {
        Overlay::build(base.as_deref(), &bytes, &for_build, &path, &scratch)
    })
    .await
    .map_err(|e| e.to_string())??;
    state
        .directory_gauges
        .count(&state.directory_gauges.overlay_builds);
    state.directory_gauges.add(
        &state.directory_gauges.overlay_build_ms_total,
        started.elapsed().as_millis() as usize,
    );

    let built = Arc::new(Built {
        overlay,
        manifest,
        deltas: unfolded,
        touched: std::sync::Mutex::new(std::time::Instant::now()),
    });
    state.directory_overlays.store(class.clone(), built.clone());
    Ok(built)
}
