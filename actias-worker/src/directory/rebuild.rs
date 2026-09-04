//! The full rebuild: a class's index reassembled without opening a
//! single object file.
//!
//! Two things go wrong that no write path can fix, because both are
//! defined by a write not happening.
//!
//! An object contributes its row on the flight that settles a write, so
//! an object that never writes again never contributes one. That row is
//! missing from the index: a false negative, the one failure this design
//! refuses. And an object with a declared lifespan can expire with
//! nobody dispatching to it, so unlike `state:destroy()` no code ever
//! runs to offer its tombstone. That row stays in the index forever: a
//! false positive, survivable but wrong.
//!
//! Both are answered from metadata. Every shipping manifest carries the
//! object's settled row, and the placement store knows which identities
//! still exist, so this is a copy between things already written down.
//! One GET per object, no restores, no leases on the objects, nothing
//! woken. That is what makes this affordable enough to be the answer to
//! "the index looks wrong" rather than a last resort.
//!
//! Idempotent by construction: rows are offered at the epoch their
//! manifest names and merge under the same last-writer-wins rule as any
//! shipped row, so a rebuild cannot overwrite something newer and
//! running it twice changes nothing the first run did not.

use std::collections::{HashMap, HashSet};

use actias_worker_core::directory::delta;
use actias_worker_core::directory::repair::{self, Carried};
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::proto::node_registry::ListInstancesRequest;

use crate::directory::sync::ClassKey;
use crate::server::AppState;

/// Identities fetched per page from the placement store.
const PAGE: u32 = 500;

/// What one rebuild did, for the operator who asked for it.
#[derive(Debug, Default, PartialEq)]
pub struct Rebuilt {
    /// Identities the placement store still lists as live.
    pub live: usize,
    /// Rows recovered from manifests.
    pub rows: usize,
    /// Live objects offered an empty placeholder row because nothing
    /// has ever derived one for them (never shipped, or shipped with
    /// no row). The index learns they exist, which is what closes the
    /// identity gate; a large count here means a backfill, not a
    /// repair, is what would give them fields.
    pub without_row: usize,
    /// Rows retired because the object no longer exists.
    pub tombstones: usize,
}

/// Every live identity of one class, paged out of the placement store.
async fn live_identities(state: &AppState, class: &ClassKey) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut page = 0u32;
    loop {
        let response = state
            .registry
            .clone()
            .list_instances(ListInstancesRequest {
                project_ids: vec![class.scope_id.clone()],
                class: class.class.clone(),
                page_size: PAGE,
                page,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?
            .into_inner();

        let count = response.instances.len();
        for instance in response.instances {
            // A row with a deletion time is going away and already
            // refuses claims, so the index should stop answering with
            // it. If the deletion somehow unwinds, the recreation
            // claims a higher epoch and outranks the tombstone.
            if instance.deleted_at_ms == 0 {
                names.push(instance.name);
            }
        }
        if count < PAGE as usize {
            return Ok(names);
        }
        page += 1;
    }
}

/// Rebuilds one class's directory from manifests and live identities.
///
/// # Errors
/// Returns the store's or the registry's message. A manifest that
/// cannot be read is skipped with a warning rather than failing the
/// pass: a rebuild that recovers most of a class is worth more than one
/// that recovers none, and the next run retries what it missed.
pub async fn rebuild_class(state: &AppState, class: &ClassKey) -> Result<Rebuilt, String> {
    let names = live_identities(state, class).await?;
    let mut summary = Rebuilt {
        live: names.len(),
        ..Rebuilt::default()
    };

    let mut carried = Vec::with_capacity(names.len());
    let mut live_ids = HashSet::with_capacity(names.len());
    for name in names {
        // The placement store already chose the scope, so the identity
        // is received rather than built: this must hash to the same id
        // the shipper used, or the manifest would not be found.
        let object_id = ObjectKey::received(&class.scope_id, &class.class, &name).object_id();
        live_ids.insert(object_id.clone());

        let manifest = match state.object_store.manifest(&object_id).await {
            Ok(Some(manifest)) => manifest,
            // No manifest means nothing has ever settled for this
            // object: it exists and has never shipped. Carried with no
            // row and no epoch, so repair offers its placeholder (an
            // empty row at rev 0, epoch 0) and the index learns the
            // identity exists; without that the invariant gate would
            // reopen for it on every pass.
            Ok(None) => {
                carried.push(Carried {
                    object_id,
                    name,
                    epoch: 0,
                    deleted: false,
                    row: None,
                });
                continue;
            }
            Err(error) => {
                actias_common::tracing::warn!(
                    class = %class.class,
                    %name,
                    %error,
                    "an object manifest could not be read during a directory rebuild"
                );
                continue;
            }
        };

        carried.push(Carried {
            object_id,
            name,
            epoch: manifest.epoch,
            deleted: manifest.deleted,
            row: manifest.directory,
        });
    }

    let repaired = repair::rows_from_manifests(carried);
    summary.rows = repaired.rows.len();
    summary.without_row = repaired.without_row;
    summary.tombstones = repaired.tombstones;

    // Rows the index still answers with whose object is no longer
    // live: the expiry case, which no write path can reach.
    let mut rows = repaired.rows;
    match crate::directory::read::indexed_identities(state, class).await {
        Ok(indexed) => {
            let vanished = repair::tombstones_for_vanished(indexed, &live_ids);
            summary.tombstones += vanished.len();
            rows.extend(vanished);
        }
        Err(error) => {
            // The recovery half already succeeded; losing the
            // reconciliation half costs stale rows, not missing ones,
            // which is the survivable direction.
            actias_common::tracing::warn!(
                class = %class.class,
                %error,
                "the directory index could not be read for reconciliation"
            );
        }
    }

    if rows.is_empty() {
        actias_common::tracing::debug!(
            class = %class.class,
            live = summary.live,
            without_row = summary.without_row,
            "a directory rebuild found nothing to offer"
        );
        return Ok(summary);
    }

    rows.sort_by(|left, right| left.object_id.cmp(&right.object_id));

    // The owner's declaration rides along, because losing the manifest
    // loses the field set with it. Without it a rebuild recovers every
    // row and every entry comes back with no fields at all, since a
    // manifest with no fields generates no overlay columns: names
    // queryable, values gone, and nothing self-healing, because a
    // backfill only runs for a building field and an empty field set
    // has none.
    //
    // Not a guess: it is the same declaration the write path stamps,
    // read from the owner's contract. If the rows predate it they read
    // as building, which is exactly true of them, and the backfill is
    // what resolves that.
    let declaration = match rows.first() {
        Some(row) => {
            let key = ObjectKey::received(&class.scope_id, &class.class, &row.name);
            match crate::routing::owner_prepared(state, &key).await {
                Ok(revision) => revision.directory_spec(&class.class),
                Err(error) => {
                    actias_common::tracing::warn!(
                        class = %class.class,
                        %error,
                        "the owner's declaration could not be read; rows repair without it"
                    );
                    None
                }
            }
        }
        None => None,
    };

    let bytes = delta::encode(&rows, declaration.as_ref(), &state.object_data_dir)?;
    let name = blake3::hash(&bytes).to_hex().to_string();
    state
        .object_store
        .put_directory_delta(&class.scope_id, &class.class, &name, bytes)
        .await?;

    actias_common::tracing::info!(
        class = %class.class,
        live = summary.live,
        rows = summary.rows,
        tombstones = summary.tombstones,
        without_row = summary.without_row,
        "a directory rebuild offered a repair delta"
    );
    Ok(summary)
}

/// Reconciles the classes this node has written rows for, forever.
///
/// Far rarer than compaction: this is one GET per object, where folding
/// reads only what is already content-addressed and local. It exists to
/// bound how long a missing or ghost row can persist, not to keep the
/// index fresh, which the write path already does.
///
/// Takes the compactor's class lease rather than one of its own. Two
/// nodes rebuilding the same class would agree anyway, since identical
/// rows encode to identical bytes and therefore to one content-addressed
/// name, so the lease saves the duplicated GETs rather than preventing
/// a conflict.
/// Passes one ownership era lasts. Long enough that a class stays with
/// one node across many checks (so the work is not reshuffled for
/// nothing), short enough that a wedged node's share moves within an
/// hour at the default interval.
const ERA_EVERY: u64 = 4;

/// Which classes this node reconciles, by rendezvous over the live node
/// list.
///
/// One node per class rather than every node walking every class: the
/// lease traffic is O(classes) per interval instead of O(classes x
/// nodes) multiplied by tenant count. A node's absence
/// costs the classes it owned one interval of lateness, which a
/// backstop can afford: the crash sweep is what answers a death
/// promptly.
fn mine(node_id: &str, nodes: &[String], class: &ClassKey, era: u64) -> bool {
    if nodes.len() <= 1 {
        return true;
    }
    // Rendezvous rather than modulo: hashing (class, node) and taking
    // the maximum moves only the departed node's share when membership
    // changes, where modulo over a node index reshuffles everything.
    //
    // `era` rotates ownership slowly. Without it, sharding trusts
    // membership completely, so a wedged node (heartbeating but doing
    // no work) silently stops checking its share forever and nothing
    // notices: a dead node is covered because membership changes, a
    // live-but-stuck one is not. Rotating means its classes land on a
    // working node within a few eras. Still exactly one owner per era,
    // so the cost is unchanged.
    let winner = nodes.iter().max_by_key(|node| {
        blake3::hash(format!("{era}:{}:{}:{}", class.scope_id, class.class, node).as_bytes())
            .to_hex()
            .to_string()
    });
    winner.is_some_and(|winner| winner == node_id)
}

/// Live node ids, for the rendezvous above. An unreachable registry
/// answers empty, which makes every node take every class: late and
/// noisy beats a class nobody checks.
pub(crate) async fn live_nodes(state: &AppState) -> Vec<String> {
    match state.registry.clone().list_nodes(()).await {
        Ok(response) => {
            let mut ids: Vec<String> = response
                .into_inner()
                .nodes
                .into_iter()
                .map(|node| node.node_id)
                .collect();
            ids.sort();
            ids
        }
        Err(_) => Vec::new(),
    }
}

/// The identity checksum per class in one scope, fetched at most once
/// per pass.
///
/// `CountInstances` answers for the whole scope, so asking it per class
/// makes a project with a hundred classes issue a hundred identical
/// rpcs and read one row from each. Filled lazily rather than up front,
/// so a pass that owns no class in a scope still costs nothing there.
///
/// [`None`] for a scope the store could not answer for, which the
/// caller treats as "not known" rather than "fine".
async fn scope_counts<'a>(
    state: &AppState,
    scope_id: &str,
    seen: &'a mut HashMap<String, Option<HashMap<String, i64>>>,
) -> Option<&'a HashMap<String, i64>> {
    if !seen.contains_key(scope_id) {
        let answer = state
            .registry
            .clone()
            .count_instances(
                actias_worker_core::proto::node_registry::CountInstancesRequest {
                    project_ids: vec![scope_id.to_owned()],
                },
            )
            .await;
        let counts = match answer {
            Ok(answer) => Some(
                answer
                    .into_inner()
                    .counts
                    .into_iter()
                    .map(|count| (count.class, count.identities))
                    .collect(),
            ),
            Err(error) => {
                // The wire carries a clean message by design, so the
                // cause is in the placement store's own log.
                actias_common::tracing::warn!(
                    scope = %scope_id,
                    %error,
                    "the placement store did not answer with its identity checksums, \
                     so every class in this scope reconciles; script-service's log \
                     carries the cause"
                );
                None
            }
        };
        seen.insert(scope_id.to_owned(), counts);
    }
    seen.get(scope_id).and_then(|counts| counts.as_ref())
}

/// Whether a class needs the expensive pass, by the identity
/// invariant.
///
/// The manifest carries the checksum of the identities its base holds
/// rows for; the placement store carries the checksum of the identities
/// that exist. Equal means the index names exactly the objects that
/// exist, so the rebuild's one-GET-per-object walk buys nothing.
///
/// Comparing identities rather than counts is what removes the blind
/// spot: one row missing and one ghost leave the count untouched, and
/// nothing downstream can detect a missing row. It is also why no
/// periodic deep pass exists: there is nothing left for one to catch.
///
/// A checksum the caller could not obtain means a pass, because not
/// knowing is not the same as knowing it is fine.
fn needs_pass(store: Option<&HashMap<String, i64>>, class: &str, indexed: i64) -> bool {
    let Some(store) = store else {
        return true;
    };
    store.get(class).copied().unwrap_or(0) != indexed
}

pub async fn run(state: AppState, every: std::time::Duration) {
    let mut pass: u64 = 0;
    loop {
        // Jittered, so a cluster restarted together does not wake
        // together forever after. Up to a quarter of the interval,
        // derived from the node id so it is stable per node rather than
        // needing a random source.
        let jitter = state
            .node_identity
            .read()
            .ok()
            .and_then(|identity| identity.clone())
            .map(|node| {
                let spread = (every.as_millis() / 4).max(1) as u64;
                u64::from(blake3::hash(node.as_bytes()).as_bytes()[0]) * spread / 256
            })
            .unwrap_or(0);
        tokio::time::sleep(every + std::time::Duration::from_millis(jitter)).await;
        pass += 1;
        state.directory_gauges.count(&state.directory_gauges.passes);
        // Asked of the store, not of this node. This node's
        // `known_classes` is empty after a restart, so a pass driven by
        // it goes quiet exactly when repair is needed, which is the
        // opposite of what reconciliation is for.
        //
        // The store cannot name a class whose prefix is gone entirely.
        // That is disaster recovery, not reconciliation, and belongs to
        // an operator verb naming the class. The placement store looks
        // like the more complete source and is not: an unscoped
        // `ListInstances` matches no rows by design, since defaulting to
        // every project's objects is wrong for a multi-tenant listing.
        let classes = match state.object_store.directory_classes().await {
            Ok(classes) => classes,
            Err(error) => {
                actias_common::tracing::warn!(
                    %error,
                    "the classes to reconcile could not be listed; retrying on the next pass"
                );
                continue;
            }
        };

        let nodes = live_nodes(&state).await;
        // One count rpc per scope for the whole pass, filled as classes
        // in that scope are reached.
        let mut counted: HashMap<String, Option<HashMap<String, i64>>> = HashMap::new();

        // A repair pass that silently does nothing is the failure mode
        // that hides every bug in it, so each pass says what it found
        // to look at and each skip below says why it skipped.
        actias_common::tracing::debug!(
            classes = classes.len(),
            nodes = nodes.len(),
            "a directory reconciliation pass began"
        );

        for (scope_id, class) in classes {
            let class = ClassKey { scope_id, class };
            let node_id = match state.node_identity.read() {
                Ok(identity) => identity.clone(),
                Err(_) => None,
            };
            let Some(node_id) = node_id else {
                actias_common::tracing::debug!(
                    "reconciliation skipped: this node has not finished registering"
                );
                continue;
            };

            // One node per class. Checked before the lease, so a class
            // that is not ours costs no rpc at all: that is what turns
            // O(classes x nodes) leases per interval into O(classes).
            if !mine(&node_id, &nodes, &class, pass / ERA_EVERY) {
                continue;
            }

            // The identity invariant, checked before the expensive
            // walk. A healthy class costs one comparison instead of one
            // GET per object, which is what lets the interval stay
            // short without the cost growing with tenants.
            let recorded = state
                .object_store
                .directory_manifest(&class.scope_id, &class.class)
                .await
                .ok()
                .flatten();
            state
                .directory_gauges
                .count(&state.directory_gauges.gate_checks);
            let repair = match recorded.as_ref().and_then(|manifest| manifest.identities) {
                Some(indexed) => {
                    let counts = scope_counts(&state, &class.scope_id, &mut counted).await;
                    needs_pass(counts, &class.class, indexed)
                }
                None => true,
            };
            // A healthy class can still owe a backfill: a publish added
            // a field and the rows predate it. That needs the pass but
            // not the walk, so it opens the lease without the rebuild.
            // Judged from the owner's current declaration, not only the
            // recorded set, because a quiet class has had no write to
            // carry the declaration to a fold yet, and waiting for one
            // would leave the field building until an object happened
            // to write.
            let building = !crate::directory::read::declared_manifest(
                &state,
                &class,
                recorded.as_ref().unwrap_or(&Default::default()),
                true,
            )
            .await
            .building()
            .is_empty();
            if !repair && !building {
                continue;
            }
            if repair {
                state
                    .directory_gauges
                    .count(&state.directory_gauges.gate_opened);
            }

            let lease = state
                .registry
                .clone()
                .acquire_lease(
                    actias_worker_core::proto::node_registry::AcquireLeaseRequest {
                        object_id: crate::directory::compact::lease_id(&class),
                        node_id,
                        ..Default::default()
                    },
                )
                .await;
            // Someone else holds the class, or the registry is
            // unreachable. Either way the next pass retries.
            let held = lease.is_ok_and(|lease| lease.into_inner().acquired);
            if !held {
                actias_common::tracing::debug!(
                    class = %class.class,
                    "reconciliation skipped: another node holds the class"
                );
                continue;
            }

            reconcile_class(&state, &class, repair).await;
        }
    }
}

/// One class reconciled under a lease the caller already holds: rebuild
/// from manifests, build any field a newer publish left behind, fold.
///
/// Every step reports its own failure and none stops the next: a
/// rebuild that recovers most of a class is worth more than one that
/// recovers none, and each step is retried by the next pass anyway.
async fn reconcile_class(state: &AppState, class: &ClassKey, repair: bool) -> Rebuilt {
    let gauges = &state.directory_gauges;
    // The walk only when the invariant asked for it; a pass opened by
    // a building field alone goes straight to the backfill.
    let rebuilt = if !repair {
        Rebuilt::default()
    } else {
        match rebuild_class(state, class).await {
            Ok(rebuilt) => {
                gauges.count(&gauges.rebuilds);
                gauges.add(&gauges.rebuilt_rows, rebuilt.rows);
                gauges.add(&gauges.placeholder_rows, rebuilt.without_row);
                rebuilt
            }
            Err(error) => {
                gauges.count(&gauges.rebuild_failures);
                actias_common::tracing::warn!(
                    class = %class.class,
                    %error,
                    "a directory rebuild failed; retrying on the next pass"
                );
                Rebuilt::default()
            }
        }
    };

    // Fields a newer publish left behind, rebuilt from restored copies.
    // Under the same lease, and before the fold, so the rows it offers
    // land in the same pass that folds them. Costs one manifest read
    // when nothing is building, which is the common case.
    match crate::directory::backfill::backfill_class(state, class).await {
        Ok(done) if done.rebuilt > 0 || done.skipped > 0 => {
            gauges.add(&gauges.backfilled_rows, done.rebuilt);
            gauges.add(&gauges.backfill_skipped, done.skipped);
            gauges
                .backfill_remaining
                .store(done.remaining as i64, std::sync::atomic::Ordering::Relaxed);
            actias_common::tracing::info!(
                class = %class.class,
                rebuilt = done.rebuilt,
                skipped = done.skipped,
                remaining = done.remaining,
                "a directory backfill ran"
            );
        }
        Ok(_) => {}
        Err(error) => {
            actias_common::tracing::warn!(
                class = %class.class,
                %error,
                "a directory backfill failed; the next pass retries"
            );
        }
    }

    // Fold here, under the lease the caller already holds (a claim by
    // the incumbent re-grants). Compaction's own discovery is
    // node-scoped via `sync.record`, so a repair delta on a class
    // nobody is writing to would otherwise never fold: readers stay
    // correct reading unfolded deltas, but pay their materialization
    // forever. Unconditional because a quiet class may hold deltas from
    // an earlier pass; a class with nothing fresh costs one list.
    match crate::directory::compact::compact_class(state, class, true).await {
        Ok(folded) if folded => {
            actias_common::tracing::info!(
                class = %class.class,
                "the reconciliation pass folded its class's deltas"
            );
        }
        Ok(_) => {}
        Err(error) => {
            gauges.count(&gauges.fold_failures);
            actias_common::tracing::warn!(
                class = %class.class,
                %error,
                "the post-rebuild fold failed; the compactor or the next pass retries"
            );
        }
    }

    rebuilt
}

/// One class reconciled because an operator asked, by name.
///
/// The background pass discovers classes by listing the store, so a
/// class whose prefix is gone entirely is invisible to it: nothing
/// names it. This is the path for that, and it is the same work the
/// pass does, taken under the same lease. [`None`] when another node
/// holds the class and is already doing it.
///
/// # Errors
/// Returns the registry's message when this node cannot ask for the
/// lease at all.
pub async fn rebuild_on_demand(
    state: &AppState,
    class: &ClassKey,
) -> Result<Option<Rebuilt>, String> {
    let node_id = state
        .node_identity
        .read()
        .map_err(|_| "this node's identity is unreadable".to_owned())?
        .clone()
        .ok_or_else(|| "this node has not finished registering".to_owned())?;

    let held = state
        .registry
        .clone()
        .acquire_lease(
            actias_worker_core::proto::node_registry::AcquireLeaseRequest {
                object_id: crate::directory::compact::lease_id(class),
                node_id,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| error.message().to_owned())?
        .into_inner()
        .acquired;
    if !held {
        return Ok(None);
    }

    let rebuilt = reconcile_class(state, class, true).await;
    actias_common::tracing::info!(
        class = %class.class,
        live = rebuilt.live,
        rows = rebuilt.rows,
        tombstones = rebuilt.tombstones,
        "an operator rebuilt a directory class"
    );
    Ok(Some(rebuilt))
}

/// The class lease is held by whichever node folds the class, for as
/// long as that node lives, so an operator's rebuild that lands on any
/// other node loses its claim. The rebuild is the holder's to run, one
/// hop away, exactly as an object call reaches its holder; [`None`]
/// when the holder cannot be reached, which the caller answers as
/// "held elsewhere".
pub async fn forward_to_holder(
    state: &AppState,
    class: &ClassKey,
    request: actias_worker_core::proto::worker_data::DirectoryRebuild,
) -> Option<actias_worker_core::proto::worker_data::DirectoryRebuilt> {
    let lease = state
        .registry
        .clone()
        .get_lease(actias_worker_core::proto::node_registry::GetLeaseRequest {
            object_id: crate::directory::compact::lease_id(class),
        })
        .await
        .ok()?
        .into_inner();
    let own = state.node_identity.read().ok()?.clone()?;
    if lease.node_id == own {
        return None;
    }
    let address = crate::directory::route::address_of(state, &lease.node_id)
        .await
        .ok()?;
    let mut client = crate::data_plane::peer_client(state, &address).await.ok()?;
    match client
        .rebuild_directory(crate::directory::route::hopped(state, request))
        .await
    {
        Ok(response) => Some(response.into_inner()),
        Err(error) => {
            actias_common::tracing::debug!(
                class = %class.class,
                holder = %lease.node_id,
                %error,
                "the class's holder did not run the rebuild"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> ClassKey {
        ClassKey {
            scope_id: "scope".to_owned(),
            class: name.to_owned(),
        }
    }

    #[test]
    fn the_invariant_gate_opens_on_a_mismatch_and_on_not_knowing() {
        use actias_worker_core::directory::identity::checksum;
        let a = "4a4e19c3d7b123c9d699716b54e8b1127e13d7f5135c10f0ccbd2d4ec2f1a163";
        let b = "18f9afd487df8a82e6dbe8ca930fef6fa5e431e422305ec2623cd6c9d44dd3f6";
        let c = "98631e9a7490b580a26dcdeb18793fff77432272eb5eda36887bf8e4716f7b26";

        let store: HashMap<String, i64> = [("Auction".to_owned(), checksum([a, b]))]
            .into_iter()
            .collect();
        assert!(
            !needs_pass(Some(&store), "Auction", checksum([b, a])),
            "a healthy class costs one comparison and nothing else, \
             whatever order each side folded in"
        );
        assert!(
            needs_pass(Some(&store), "Auction", checksum([a])),
            "a missing row, the fatal direction"
        );
        assert!(
            needs_pass(Some(&store), "Auction", checksum([a, b, c])),
            "a ghost row left by an expiry"
        );
        // The case counts cannot see: one row missing, one ghost, same
        // count on both sides.
        assert!(
            needs_pass(Some(&store), "Auction", checksum([a, c])),
            "one missing and one ghost is exactly what the count \
             invariant read as healthy"
        );
        // A class the scope does not mention has no identities, so any
        // rows it holds are ghosts.
        assert!(needs_pass(Some(&store), "Gone", checksum([a])));
        assert!(
            needs_pass(None, "Auction", checksum([a, b])),
            "a checksum nobody could fetch means a pass: not knowing is \
             not the same as knowing it is fine"
        );
    }

    #[test]
    fn exactly_one_node_owns_each_class() {
        let nodes: Vec<String> = (0..5).map(|n| format!("node-{n}")).collect();
        for name in ["Auction", "Account", "Floor", "Probe", "Registrar"] {
            let owners = nodes
                .iter()
                .filter(|node| mine(node, &nodes, &class(name), 0));
            assert_eq!(
                owners.count(),
                1,
                "'{name}' must be reconciled by one node, or the lease traffic \
                 multiplies by cluster size again"
            );
        }
    }

    #[test]
    fn a_lone_node_owns_everything() {
        // Before registration completes, and in a single-node cluster,
        // skipping would leave classes nobody checks.
        assert!(mine("only", &["only".to_owned()], &class("Auction"), 0));
        assert!(mine("only", &[], &class("Auction"), 0));
    }

    /// The wedged-node cover: a class must not stay with one node
    /// forever, or a node that heartbeats while doing nothing strands
    /// its whole share.
    #[test]
    fn ownership_rotates_across_eras() {
        let nodes: Vec<String> = (0..4).map(|n| format!("node-{n}")).collect();
        let c = class("Auction");

        let owner = |era: u64| -> String {
            nodes
                .iter()
                .find(|node| mine(node, &nodes, &c, era))
                .cloned()
                .unwrap_or_default()
        };

        let seen: std::collections::HashSet<String> = (0..24).map(owner).collect();
        assert!(
            seen.len() > 1,
            "one class owned by one node forever is exactly the wedged-node hole"
        );

        // Still exactly one owner within an era: rotation must not turn
        // into every node checking every class.
        for era in 0..8 {
            let owners = nodes.iter().filter(|node| mine(node, &nodes, &c, era));
            assert_eq!(owners.count(), 1);
        }
    }

    #[test]
    fn losing_a_node_moves_only_its_own_share() {
        // Rendezvous rather than modulo over an index: this is the
        // whole reason for the hash-max. Under modulo, removing one
        // node reshuffles nearly every class, and a reshuffle means
        // every moved class waits a fresh interval.
        let before: Vec<String> = (0..6).map(|n| format!("node-{n}")).collect();
        let after: Vec<String> = before.iter().skip(1).cloned().collect();
        let classes: Vec<ClassKey> = (0..300).map(|n| class(&format!("Class{n}"))).collect();

        let owner = |nodes: &[String], c: &ClassKey| -> String {
            nodes
                .iter()
                .find(|node| mine(node, nodes, c, 0))
                .cloned()
                .unwrap_or_default()
        };

        let moved = classes
            .iter()
            .filter(|c| owner(&before, c) != owner(&after, c))
            .count();
        // Only what node-0 held should move: about a sixth.
        assert!(
            moved < classes.len() / 3,
            "removing 1 of 6 nodes moved {moved} of {} classes; rendezvous should \
             move only the departed node's share",
            classes.len()
        );
    }
}
