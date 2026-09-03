//! Object call routing: from an identity to the owner's current code to
//! the pinned vm, with forwarding to the lease holder and replica-backed
//! reads. The http surface in [`crate::server`] only decodes transports
//! into these calls; everything about placement and code resolution lives
//! here.

use std::sync::Arc;

use actias_common::logging::script_log_channel;
use actias_worker_core::extensions::log::LogPublisher;
use actias_worker_core::extensions::objects::{
    DirectoryAnswer, DirectoryLister, DirectoryRequest, ObjectRouter, ObjectTarget,
};
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::proto::node_registry::AcquireLeaseRequest;
use actias_worker_core::proto::script_service::FindScriptRequest;
use actias_worker_core::proto::script_service::GetRevisionRequest;
use actias_worker_core::proto::script_service::ResolveClassOwnerRequest;
use actias_worker_core::proto::script_service::Script;
use actias_worker_core::proto::script_service::find_script_request::Query;
use actias_worker_core::runtime::{ActiasRuntime, PreparedRevision};

use crate::server::{AppState, FOREIGN_REVISION};

/// One revision prepared through the cache: the manifest travels over
/// grpc, the bytes come from the blob store by hash, and the compiled
/// result is shared by every request that runs it.
pub async fn cached_revision(
    state: &AppState,
    script: Script,
    revision_id: String,
) -> Result<Arc<PreparedRevision>, Arc<anyhow::Error>> {
    state
        .caches
        .revisions
        .try_get_with(revision_id.clone(), {
            let mut client = state.clients.script.clone();
            let blobs = state.blobs.clone();
            async move {
                let mut revision = client
                    .get_revision(GetRevisionRequest {
                        id: revision_id,
                        with_bundle: true,
                        manifest_only: true,
                    })
                    .await?
                    .into_inner();

                // A preview could name any revision; only this script's may
                // serve under its identifier. Failed loads are not cached.
                if revision.script_id != script.id {
                    anyhow::bail!(FOREIGN_REVISION);
                }

                if let Some(bundle) = revision.bundle.as_mut() {
                    for file in &mut bundle.files {
                        if file.content.is_empty() && !file.hash.is_empty() {
                            file.content = blobs.get(&file.hash).await?.as_ref().clone();
                        }
                    }
                }

                Ok::<_, anyhow::Error>(Arc::new(PreparedRevision::prepare(script, revision)?))
            }
        })
        .await
}

/// The code an object identity runs: the owner script's current revision,
/// always. The owner comes from the project's current contracts (the
/// script-service falls back to the instance directory for orphaned
/// data); `__cron` scopes to its script, which the key's scope already
/// names. Cached on the pointer ttl, so a republish or an owner move
/// propagates like any pointer.
pub async fn owner_prepared(
    state: &AppState,
    key: &ObjectKey,
) -> Result<Arc<PreparedRevision>, String> {
    state
        .caches
        .owners
        .try_get_with(key.to_string(), {
            let state = state.clone();
            let key = key.clone();
            async move {
                let script_id = if key.is_cron() {
                    key.scope().to_owned()
                } else {
                    state
                        .clients
                        .script
                        .clone()
                        .resolve_class_owner(ResolveClassOwnerRequest {
                            project_id: key.scope().to_owned(),
                            class: key.class().to_owned(),
                            name: key.name().to_owned(),
                        })
                        .await
                        .map_err(|e| match e.code() {
                            tonic::Code::NotFound => {
                                "No script in the project owns this object.".to_owned()
                            }
                            _ => e.to_string(),
                        })?
                        .into_inner()
                        .script_id
                };

                let script = state
                    .clients
                    .script
                    .clone()
                    .query_script(FindScriptRequest {
                        query: Some(Query::Id(script_id)),
                    })
                    .await
                    .map_err(|e| e.to_string())?
                    .into_inner();
                let revision_id = script
                    .current_revision_id
                    .clone()
                    .ok_or_else(|| "The owner script has no published revision.".to_owned())?;

                cached_revision(&state, script, revision_id)
                    .await
                    .map_err(|e| e.to_string())
            }
        })
        .await
        .map_err(|e: Arc<String>| e.as_ref().clone())
}

/// Everything routing an object method call needs; one per node, shared
/// by request vms and pinned vms alike, so objects call objects through
/// exactly the machinery requests use. The prepared revision is the
/// caller's context (whose project and script derive identities); the
/// code an object runs is always the owner's current revision, resolved
/// per identity through [`owner_prepared`].
pub struct ObjectRouting {
    state: AppState,
    /// The caller context: whose project and script derive identities.
    pub(crate) prepared: Arc<PreparedRevision>,
    /// Set for a request vm: local object writes answer at commit and
    /// their gates collect here, to be settled once before the response
    /// leaves. Object and connection vms hold every answer as before.
    deferred: Option<Arc<actias_worker_core::objects::PendingGates>>,
}

/// Why an object could not be made resident here.
pub enum ResolveError {
    /// A live incumbent holds the lease; forward the call to it.
    Elsewhere(String),
    Other(String),
}

/// Why a routed call failed, typed so the transport can tell a stale
/// home apart from a real failure without reading message text.
pub enum RouteError {
    /// This node neither holds the object nor may forward; the caller's
    /// view of the home is stale and it should re-resolve.
    WrongHome {
        holder: String,
    },
    Failed(String),
}

impl From<String> for RouteError {
    fn from(message: String) -> Self {
        RouteError::Failed(message)
    }
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::WrongHome { holder } => {
                write!(f, "Object is homed on {holder}; this node cannot serve it.")
            }
            RouteError::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl RouteError {
    /// The message a vm or log sees; the shape collapses deliberately.
    pub fn into_message(self) -> String {
        match self {
            RouteError::WrongHome { holder } => {
                format!("Object is homed on {holder}, but this call may not forward again.")
            }
            RouteError::Failed(message) => message,
        }
    }
}

/// Why one forward hop came back empty.
enum ForwardError {
    /// The home moved or died; invalidate and re-resolve once.
    StaleHome(String),
    /// The object answered; this is its own failure, passed through.
    Call(String),
}

impl ObjectRouting {
    /// Routing for one prepared revision over this node's shared pieces.
    pub fn new(state: &AppState, prepared: Arc<PreparedRevision>) -> Arc<Self> {
        Arc::new(Self {
            state: state.clone(),
            prepared,
            deferred: None,
        })
    }

    /// Routing for a request vm, which settles its writes' gates itself
    /// (see [`actias_worker_core::objects::PendingGates`]).
    pub fn deferring(
        state: &AppState,
        prepared: Arc<PreparedRevision>,
        gates: Arc<actias_worker_core::objects::PendingGates>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: state.clone(),
            prepared,
            deferred: Some(gates),
        })
    }

    /// Wraps this routing as the closure vms carry in app data.
    pub(crate) fn as_router(self: &Arc<Self>) -> ObjectRouter {
        let this = self.clone();
        Arc::new(move |target: ObjectTarget| {
            let this = this.clone();
            Box::pin(async move { this.route(target).await })
        })
    }

    /// Wraps this routing as the reading closure, the read-path sibling
    /// of [`Self::as_router`].
    ///
    /// The vm names a class and a predicate; the scope is never its to
    /// choose, so it comes from the revision the vm is running. That is
    /// also the fence: a script can only ever read classes in its own
    /// project, because it cannot spell a scope at all.
    ///
    /// Answered from this node with no lease and nothing woken. A
    /// listing costs one local overlay read wherever the request
    /// landed; a verified read adds one small manifest fetch per
    /// candidate, and still restores nothing on the common path.
    pub(crate) fn as_lister(self: &Arc<Self>) -> DirectoryLister {
        let this = self.clone();
        Arc::new(move |request: DirectoryRequest| {
            let this = this.clone();
            Box::pin(async move {
                let class = crate::directory::sync::ClassKey {
                    scope_id: this.prepared.script.project_id.clone(),
                    class: request.class,
                };
                let query = actias_worker_core::directory::overlay::Query {
                    where_: request.where_,
                    order: request.order,
                    limit: request.limit,
                    cursor: request.cursor,
                };
                // Routed to the class's reader like a console query, so
                // a script's listing does not materialize the class on
                // whichever node happened to run it.
                if request.verified {
                    crate::directory::route::visit(&this.state, &class, query)
                        .await
                        .map(|(page, _)| DirectoryAnswer::Visited(page))
                } else {
                    crate::directory::route::list(&this.state, &class, query)
                        .await
                        .map(|(page, _)| DirectoryAnswer::Listed(page))
                }
            })
        })
    }

    /// The lease claim for one identity, spoken as this node. The claim
    /// carries the key's preimage plus the owner script as directory
    /// metadata.
    async fn claim_lease(
        &self,
        key: &ObjectKey,
        owner: &PreparedRevision,
    ) -> Result<actias_worker_core::proto::node_registry::Lease, String> {
        let node_id = self
            .state
            .node_identity
            .read()
            .expect("no poisoned lock")
            .clone()
            .ok_or_else(|| "This node has not finished registering; try again.".to_owned())?;

        Ok(self
            .state
            .registry
            .clone()
            .acquire_lease(AcquireLeaseRequest {
                object_id: key.object_id(),
                node_id,
                scope_id: key.scope().to_owned(),
                class: key.class().to_owned(),
                name: key.name().to_owned(),
                script_id: owner.script.id.clone(),
                // The contract's declared lifespan; every claim restates
                // it, so touch refreshes and policy changes apply on the
                // next touch. The creator stamp is not filled in here.
                expire_secs: owner.expire_secs_for(key.class()).unwrap_or(0),
                created_by: String::new(),
            })
            .await
            .map_err(|e| e.to_string())?
            .into_inner())
    }

    /// The object's live mailbox, spawning (and thereby reviving) it if
    /// needed: owner resolution, lease claim, vm, storage, all of it. The
    /// cold-alarm sweep calls this too; making an object resident is all a
    /// wake takes, because the spawned task re-arms its persisted alarm
    /// itself.
    ///
    /// The vm registry is keyed by object identity and marked with the
    /// owner's current revision, so a republish gets fresh code on first
    /// touch instead of stale vms accumulating.
    ///
    /// A refusal surfaces the incumbent, so the caller can forward there
    /// instead of failing.
    pub async fn resolve_handle(
        self: &Arc<Self>,
        key: &ObjectKey,
        use_cache: bool,
    ) -> Result<actias_worker_core::objects::ObjectHandle, ResolveError> {
        // The code the object runs is the owner's current revision,
        // whoever is calling; resolved before the lease so the claim can
        // record the owner in the directory.
        let owner = owner_prepared(&self.state, key)
            .await
            .map_err(ResolveError::Other)?;

        // A non-resident object needs the lease before anything spawns;
        // a resident one already holds it (leases live as long as we do).
        // A cached refusal answers before anything claims: a junk name
        // must not recreate the directory row the rollback just removed.
        if let Some(refusal) = self.state.admit_refusals.get(&key.object_id()).await {
            return Err(ResolveError::Other(refusal));
        }

        let mut fresh = false;
        if !self.state.objects.is_resident(&key.to_string()).await {
            // The holder cache spares the placement store: a repeat of a
            // misrouted call forwards straight from memory. A wrong entry
            // costs one wrong-home hop, which invalidates it.
            let object_id = key.object_id();
            if use_cache && let Some(holder) = self.state.holders.get(&object_id).await {
                let own = self
                    .state
                    .node_identity
                    .read()
                    .ok()
                    .and_then(|guard| guard.clone());
                if own.as_deref() != Some(holder.as_str()) {
                    return Err(ResolveError::Elsewhere(holder));
                }
            }
            // The placement rung: a cold object whose last replicas
            // include a live node comes up there, on its own bytes, one
            // hop away. Only on the first pass, so a forwarded call
            // claims where it lands and two nodes never trade it.
            if use_cache
                && self.state.replica_quorum > 0
                && let Some(replica) = self.replica_home(&object_id).await
            {
                return Err(ResolveError::Elsewhere(replica));
            }
            let lease = self
                .claim_lease(key, &owner)
                .await
                .map_err(ResolveError::Other)?;
            if !lease.acquired {
                self.state
                    .holders
                    .insert(object_id, lease.node_id.clone())
                    .await;
                return Err(ResolveError::Elsewhere(lease.node_id));
            }
            self.state.holders.invalidate(&object_id).await;
            // This claim is the one that can create the identity; the
            // factory's re-claim always finds the row it made. Freshness
            // travels, or the admission gate would never see it.
            fresh = lease.fresh;
        }

        self.resolve_local(key, owner, fresh)
            .await
            .map_err(ResolveError::Other)
    }

    /// A live node other than this one that the object's manifest names
    /// as a replica, when the object is not held anywhere; [`None`]
    /// means claim here.
    async fn replica_home(&self, object_id: &str) -> Option<String> {
        let manifest = self.state.object_store.manifest(object_id).await.ok()??;
        if manifest.replicas.is_empty() || manifest.deleted {
            return None;
        }
        let own = self
            .state
            .node_identity
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        if manifest
            .replicas
            .iter()
            .any(|node| own.as_deref() == Some(node.as_str()))
        {
            // Our own bytes: claim here.
            return None;
        }
        let live = crate::directory::rebuild::live_nodes(&self.state).await;
        manifest
            .replicas
            .into_iter()
            .find(|node| live.contains(node))
    }

    /// The spawn itself, lease already settled (or re-settled by the
    /// factory for the resident-revision-bump edge, where it is our own).
    async fn resolve_local(
        self: &Arc<Self>,
        key: &ObjectKey,
        owner: Arc<PreparedRevision>,
        first_claim_fresh: bool,
    ) -> Result<actias_worker_core::objects::ObjectHandle, String> {
        let routing = self.clone();
        // The hash is the object's platform-wide id: the lease key in the
        // placement store and the file name on disk, because the class and
        // instance names are user-chosen text.
        let object_id = key.object_id();
        let file = self.state.object_data_dir.join(key.db_file_name());
        let identity = key.clone();
        let marker = owner.revision_id.clone();

        let handle = self
            .state
            .objects
            .get_or_spawn(&key.to_string(), &marker, || async move {
                // A recently refused name answers from the cache: junk
                // identities cost one vm build per pointer ttl, not one
                // per call.
                if let Some(refusal) = routing.state.admit_refusals.get(&object_id).await {
                    return Err(mlua::Error::RuntimeError(refusal));
                }

                // One claim per residency, before anything is built: an
                // object only ever lives where its lease is held, which is
                // what makes a second node refusing to serve it correct.
                let lease = routing
                    .claim_lease(&identity, &owner)
                    .await
                    .map_err(mlua::Error::RuntimeError)?;
                if !lease.acquired {
                    return Err(mlua::Error::RuntimeError(
                        "Object is homed on another node.".to_owned(),
                    ));
                }

                // The store is the truth whenever anyone else held the
                // object since this node last did. A missing file means
                // never hosted (or a lost volume); an existing file can
                // still be stale: this node hosted the object once, it
                // rehomed elsewhere and wrote, and now the lease is back.
                // The sidecar records the epoch of this node's last
                // residency; a store manifest with a newer epoch wins.
                let resident_epoch = read_resident_epoch(&file);
                let manifest = match routing.state.object_store.manifest(&object_id).await {
                    Ok(manifest) => manifest,
                    Err(error) if file.exists() => {
                        // Store unreachable but the file is here: serving
                        // it keeps today's availability posture; the next
                        // claim re-checks.
                        actias_common::tracing::warn!(
                            object_id,
                            %error,
                            "snapshot manifest unreadable; serving the local file"
                        );
                        None
                    }
                    Err(error) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "The object's snapshot could not be checked: {error}"
                        )));
                    }
                };
                let manifest_epoch = manifest.as_ref().map(|m| m.epoch);
                if manifest_epoch.is_some_and(|shipped| shipped > resident_epoch) || !file.exists()
                {
                    // The replicas named by the manifest hold the tail the
                    // store may not have yet; a quorum of them is the copy
                    // to take over from, and the store is the fallback.
                    let mut laid = false;
                    if let Some(manifest) = manifest.as_ref()
                        && !manifest.deleted
                    {
                        match crate::objects::takeover::from_replicas(
                            &routing.state,
                            &object_id,
                            manifest,
                            lease.epoch,
                            &file,
                        )
                        .await
                        {
                            Ok(true) => laid = true,
                            Ok(false) => {}
                            Err(error) => actias_common::tracing::warn!(
                                object_id,
                                %error,
                                "takeover from a replica failed; restoring from the store"
                            ),
                        }
                    }
                    if !laid {
                        match routing.state.object_store.restore(&object_id, &file).await {
                            Ok(true) => {
                                actias_common::tracing::info!(
                                    object_id,
                                    "object restored from store"
                                )
                            }
                            Ok(false) => {}
                            Err(error) => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "The object's snapshot could not be restored: {error}"
                                )));
                            }
                        }
                    }
                }
                write_resident_epoch(&file, lease.epoch);

                // A workflow instance replays the revision it started
                // with, the one deliberate exception to always-current;
                // every other class runs the owner's current revision.
                let workflow = identity.class() == actias_common::classes::WORKFLOW_CLASS;
                let prepared = match actias_worker_core::platform::workflow::pinned_revision(&file)
                {
                    Some(pinned) if workflow && pinned != owner.revision_id => {
                        cached_revision(&routing.state, owner.script.clone(), pinned)
                            .await
                            .map_err(|error| {
                                mlua::Error::RuntimeError(format!(
                                    "The run's pinned revision could not load: {error:#}"
                                ))
                            })?
                    }
                    _ => owner.clone(),
                };

                // Object logs join the owner script's production channel,
                // so `actias tail` sees them like any handler line.
                let logs = routing.state.redis.clone().map(|connection| {
                    LogPublisher::new(connection, script_log_channel(&owner.script.id))
                });

                let runtime = if workflow {
                    // The enforced-determinism profile: the shared cell is
                    // both the replay cursor and the shim source. The
                    // instance file opens before the vm builds, because
                    // `secret` declarations run during construction and
                    // must see the run's pins; the task reopens the same
                    // file afterwards.
                    let pins = actias_worker_core::platform::workflow::SecretPins::load(&file)
                        .map_err(mlua::Error::RuntimeError)?;
                    let shared =
                        Arc::new(actias_worker_core::platform::workflow::WfShared::default());
                    let runtime = ActiasRuntime::with_profile(
                        prepared.clone(),
                        routing.state.clients.kv.clone(),
                        routing.state.egress.clone(),
                        logs,
                        routing.state.secret_client.clone(),
                        None,
                        actias_worker_core::runtime::VmProfile::Workflow {
                            source: shared.clone(),
                            secret_pins: Some(Arc::new(pins)),
                        },
                    )
                    .await?;
                    runtime.set_app_data(shared);
                    runtime
                } else {
                    // A warm vm of this revision when the pool has one:
                    // an object's first call no longer pays for running
                    // the whole entry point.
                    let build_state = routing.state.clone();
                    let build_prepared = prepared.clone();
                    let build: crate::vm_pool::VmBuild = Arc::new(move || {
                        let state = build_state.clone();
                        let prepared = build_prepared.clone();
                        let logs = logs.clone();
                        Box::pin(async move {
                            ActiasRuntime::new(
                                prepared,
                                state.clients.kv.clone(),
                                state.egress.clone(),
                                logs,
                                state.secret_client.clone(),
                                None,
                            )
                            .await
                            .map_err(|error| error.to_string())
                        })
                    });
                    routing
                        .state
                        .vm_pool
                        .take(
                            crate::vm_pool::VmKey {
                                revision_id: prepared.revision_id.clone(),
                                flavor: crate::vm_pool::Flavor::Object,
                            },
                            build,
                        )
                        .await
                        .map_err(mlua::Error::RuntimeError)?
                };
                routing.state.guest_limits.apply(&runtime);
                // The admission gate, fresh identities only: existing
                // instances never re-run it, so it gates creation, not
                // access. A refusal rolls the claim back through the
                // deletion verbs and caches on the pointer ttl. It runs
                // here because it needs the built vm, and before the
                // storage open, so a refused name never owns a file.
                if (lease.fresh || first_claim_fresh) && owner.gates_admission(identity.class()) {
                    let verdict = actias_worker_core::extensions::objects::admit(
                        &runtime,
                        identity.class(),
                        identity.name(),
                    )
                    .await;
                    let admitted = matches!(verdict, Ok(Some(true)) | Ok(None));
                    if !admitted {
                        let refusal = match verdict {
                            Err(error) => error,
                            _ => format!(
                                "Class '{}' did not admit '{}'.",
                                identity.class(),
                                identity.name()
                            ),
                        };
                        let rollback = routing
                            .state
                            .registry
                            .clone()
                            .rollback_admission(
                                actias_worker_core::proto::node_registry::PurgeInstanceRequest {
                                    scope_id: identity.scope().to_owned(),
                                    class: identity.class().to_owned(),
                                    name: identity.name().to_owned(),
                                    object_id: object_id.clone(),
                                },
                            )
                            .await
                            .map_err(|e| e.to_string());
                        if let Err(error) = rollback {
                            actias_common::tracing::warn!(
                                object_id,
                                %error,
                                "admission rollback incomplete; the janitor finishes it"
                            );
                        }
                        routing
                            .state
                            .admit_refusals
                            .insert(object_id.clone(), refusal.clone())
                            .await;
                        return Err(mlua::Error::RuntimeError(refusal));
                    }
                }

                // The pinned vm routes its own outbound calls too; the
                // chain it hands them is what makes cycles refusable. Its
                // routing context matches the code it runs.
                let vm_routing = ObjectRouting::new(&routing.state, prepared);
                runtime.set_app_data::<ObjectRouter>(vm_routing.as_router());
                // The read-path sibling: without it `Class:list`
                // refuses inside an object exactly as it did in a
                // request handler.
                runtime.set_app_data::<DirectoryLister>(vm_routing.as_lister());
                // The pump reads this to deliver connection edges; a
                // node without the socket serving the id prunes them.
                runtime.set_app_data::<Arc<actias_worker_core::connections::ConnectionRegistry>>(
                    routing.state.connections.clone(),
                );
                // And these to deliver the edges that live elsewhere:
                // its own name to tell local from remote, and the
                // forwarder that sends each remote node one batched
                // call for everything due there.
                runtime.set_app_data(actias_worker_core::streams::LocalNode(
                    routing
                        .state
                        .node_identity
                        .read()
                        .ok()
                        .and_then(|guard| guard.clone())
                        .unwrap_or_default(),
                ));
                runtime.set_app_data::<actias_worker_core::streams::ConnectionForwarder>(
                    crate::data_plane::connection_forwarder(&routing.state),
                );
                // Durable followers batch the same way; the identity
                // names this publisher so a range entry can be read
                // from the nearest copy of its log.
                runtime.set_app_data(actias_worker_core::streams::PublisherIdentity {
                    scope: identity.scope().to_owned(),
                    class: identity.class().to_owned(),
                    name: identity.name().to_owned(),
                });
                runtime.set_app_data::<actias_worker_core::streams::ReceiveForwarder>(
                    crate::data_plane::receive_forwarder(&routing.state),
                );

                let mut storage = actias_worker_core::storage::SqliteStorage::open(&file)
                    .map_err(mlua::Error::RuntimeError)?;
                storage
                    .set_size_limit(routing.state.object_db_max_bytes)
                    .map_err(mlua::Error::RuntimeError)?;

                // The output gate: a write marks the object dirty, takes a
                // ticket, and is answered only once a flight carrying its
                // frames has written its manifest. Bursts coalesce, so
                // one flight acknowledges many writes and the gate costs
                // latency rather than throughput. The epoch fence and the
                // drain are unaffected: the drain flushes every dirty
                // object before the process leaves.
                let ship_store = routing.state.object_store.clone();
                let ship_id = object_id.clone();
                let ship_file = file.clone();
                let epoch = lease.epoch;
                let ship_state = Arc::new(tokio::sync::Mutex::new(
                    crate::objects::store::ShipState::for_epoch(lease.epoch),
                ));
                let thresholds = routing.state.ship_thresholds;
                let membership = routing.state.membership_generation.clone();
                // The residency's replica set and the fan-out to it. With
                // no other live node there is nothing to fan out to and
                // the store's manifest stays the release, as it always
                // was on a single node.
                let replicas =
                    crate::objects::fanout::choose_replicas(&routing.state, &object_id).await;
                let quorum = if replicas.is_empty() {
                    0
                } else {
                    routing.state.replica_quorum.min(replicas.len())
                };
                let fanout = (!replicas.is_empty()).then(|| {
                    crate::objects::fanout::fanout_for(
                        routing.state.clone(),
                        object_id.clone(),
                        file.clone(),
                        replicas.clone(),
                    )
                });
                let replica_gauges = routing.state.replica_store.gauges.clone();
                let fanout_short = Arc::new(std::sync::atomic::AtomicBool::new(false));
                // Settle-fed, not commit-fed: the row is offered to the
                // syncer only after the flight carrying it wrote the
                // object's manifest, so the index describes the durable
                // universe and can never name a state a crash took back.
                // The epoch comes from the lease here, because the
                // object's own file does not know it.
                let ship_sync = routing.state.directory_sync.clone();
                let ship_class = crate::directory::sync::ClassKey {
                    scope_id: identity.scope().to_owned(),
                    class: identity.class().to_owned(),
                };
                let ship_name = identity.name().to_owned();
                // The publish these rows were derived under, read from
                // the owner's contract (the code the object actually
                // runs), so the version stamped on the row and the one
                // riding the delta always name the same declaration.
                let ship_declaration = owner.directory_spec(identity.class());
                let ship_state_for_watermark = ship_state
                    .try_lock()
                    .map(|state| state.watermark())
                    .unwrap_or_default();
                let ship_fn: crate::objects::shipper::ShipFn = Arc::new(move |release| {
                    let store = ship_store.clone();
                    let object_id = ship_id.clone();
                    let file = ship_file.clone();
                    let state = ship_state.clone();
                    let syncer = ship_sync.clone();
                    let class = ship_class.clone();
                    let name = ship_name.clone();
                    let declaration = ship_declaration.clone();
                    let membership = membership.load(std::sync::atomic::Ordering::SeqCst);
                    let replication =
                        fanout
                            .clone()
                            .map(|fanout| crate::objects::store::Replication {
                                fanout,
                                node_ids: replicas.clone(),
                                quorum,
                                release: std::sync::Mutex::new(Some(release)),
                                gauges: replica_gauges.clone(),
                                short: fanout_short.clone(),
                            });
                    let gauges = replica_gauges.clone();
                    Box::pin(async move {
                        store
                            .ship(
                                &object_id,
                                epoch,
                                &file,
                                &state,
                                thresholds,
                                membership,
                                replication.as_ref(),
                            )
                            .await?;
                        // A flight the quorum did not release is released
                        // here, at the manifest, as before.
                        if replication
                            .as_ref()
                            .is_none_or(|r| r.release.lock().map(|r| r.is_some()).unwrap_or(true))
                        {
                            gauges
                                .store_releases
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        // The manifest landed, so whatever row it
                        // carries is durable and may be indexed. A
                        // class with no directory carries none, and
                        // offering nothing is the whole cost.
                        if let Ok(Some(manifest)) = store.manifest(&object_id).await
                            && let Some(snapshot) = manifest.directory
                        {
                            syncer.record(class, object_id, name, epoch, snapshot, declaration);
                        }
                        Ok(())
                    })
                });
                let ship = crate::objects::shipper::Shipper::new(
                    object_id.clone(),
                    ship_fn,
                    routing.state.ship_gauges.clone(),
                    routing.state.ship_limits.clone(),
                );
                routing
                    .state
                    .shippers
                    .lock()
                    .expect("no poisoned lock")
                    .insert(object_id.clone(), ship.clone());
                routing
                    .state
                    .ship_states
                    .lock()
                    .expect("no poisoned lock")
                    .insert(object_id.clone(), ship_state_for_watermark);
                let ack_gate = routing.state.ack_gate;
                let after_write: actias_worker_core::objects::AfterWrite = Arc::new(move || {
                    // Marking here rather than inside the future is what
                    // lets platform work with no caller drop the wait and
                    // keep the shipping.
                    let ticket = ship.mark_and_ticket();
                    Box::pin(async move { ticket.wait(ack_gate).await })
                });

                let alarm_sync = alarm_mirror(&routing.state, &object_id, &identity.to_string());

                // The deletion sequence, in order: the tombstone is the
                // commit point, the store marker fences any zombie ship,
                // the local files go (unlinking beside the task's open
                // connection is safe; space frees on close), and the
                // purge frees the name for a fresh life.
                let destroy_state = routing.state.clone();
                let destroy_id = object_id.clone();
                let destroy_key = identity.clone();
                let destroy_file = file.clone();
                // Cloned up front: the closure is Fn and may run more
                // than once, so it cannot move the identity it names.
                let destroy_class = crate::directory::sync::ClassKey {
                    scope_id: identity.scope().to_owned(),
                    class: identity.class().to_owned(),
                };
                let destroy_name = identity.name().to_owned();
                let destroy: actias_worker_core::objects::DestroyFn = Arc::new(move || {
                    let state = destroy_state.clone();
                    let object_id = destroy_id.clone();
                    let key = destroy_key.clone();
                    let file = destroy_file.clone();
                    let class = destroy_class.clone();
                    let name = destroy_name.clone();
                    Box::pin(async move {
                        let deleted = state
                            .registry
                            .clone()
                            .delete_instance(
                                actias_worker_core::proto::node_registry::DeleteInstanceRequest {
                                    scope_id: key.scope().to_owned(),
                                    class: key.class().to_owned(),
                                    name: key.name().to_owned(),
                                    object_id: object_id.clone(),
                                    only_if_expired: false,
                                },
                            )
                            .await
                            .map_err(|e| e.to_string())?
                            .into_inner();
                        // Already tombstoned means another deleter (the
                        // sweep, an external delete) wrote the marker;
                        // this run only clears residue and purges.
                        if deleted.tombstoned {
                            state
                                .object_store
                                .mark_deleted(&object_id, deleted.epoch)
                                .await?;
                            // The directory learns of the death at the
                            // bumped epoch, so the tombstone outranks
                            // every row of the life before it.
                            state.directory_sync.record_destroyed(
                                class.clone(),
                                object_id.clone(),
                                name.clone(),
                                deleted.epoch,
                            );
                        }

                        let _ = tokio::fs::remove_file(&file).await;
                        let mut wal = file.as_os_str().to_owned();
                        wal.push("-wal");
                        let _ = tokio::fs::remove_file(std::path::PathBuf::from(wal)).await;
                        let mut shm = file.as_os_str().to_owned();
                        shm.push("-shm");
                        let _ = tokio::fs::remove_file(std::path::PathBuf::from(shm)).await;
                        let _ = tokio::fs::remove_file(file.with_extension("epoch")).await;

                        state
                            .shippers
                            .lock()
                            .expect("no poisoned lock")
                            .remove(&object_id);
                        state
                            .ship_states
                            .lock()
                            .expect("no poisoned lock")
                            .remove(&object_id);
                        state
                            .registry
                            .clone()
                            .purge_instance(
                                actias_worker_core::proto::node_registry::PurgeInstanceRequest {
                                    scope_id: key.scope().to_owned(),
                                    class: key.class().to_owned(),
                                    name: key.name().to_owned(),
                                    object_id: object_id.clone(),
                                },
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                        state.holders.invalidate(&object_id).await;
                        Ok(())
                    })
                });

                // Only classes with a lifespan restate their claim; the
                // re-claim is the same verb a revival runs, so the stamp
                // and the policy stay one code path.
                let keep_claimed = owner.expire_secs_for(identity.class()).map(|_| {
                    let keep_routing = routing.clone();
                    let keep_key = identity.clone();
                    let keep_owner = owner.clone();
                    let closure: actias_worker_core::objects::KeepClaimed = Arc::new(move || {
                        let routing = keep_routing.clone();
                        let key = keep_key.clone();
                        let owner = keep_owner.clone();
                        tokio::spawn(async move {
                            if let Err(error) = routing.claim_lease(&key, &owner).await {
                                actias_common::tracing::debug!(
                                    %error,
                                    "residency refresh claim failed"
                                );
                            }
                        });
                    });
                    closure
                });

                Ok((
                    runtime,
                    actias_worker_core::objects::TaskOptions {
                        call_budget: Some(routing.state.guest_limits.wall_secs),
                        directory_budget_ms: Some(routing.state.directory_eval_budget_ms),
                        storage: Some(storage),
                        hibernate_after: Some(routing.state.object_idle_after),
                        after_write: Some(after_write),
                        alarm_sync: Some(alarm_sync),
                        queue: routing.state.queue_policy.clone(),
                        destroy: Some(destroy),
                        keep_claimed,
                    },
                ))
            })
            .await
            .map_err(|e| e.to_string())?;
        // The residency's registries let go with its task: once the
        // mailbox has closed and the last flight has settled, the shipper
        // and its state are forgotten, so a node whose objects come and
        // go holds nothing for the ones that went. Bounded, so a flight
        // that never settles cannot pin an entry forever.
        {
            let registries = self.state.clone();
            let registry_id = key.object_id();
            let watched = handle.clone();
            tokio::spawn(async move {
                watched.ended().await;
                let shipper = registries
                    .shippers
                    .lock()
                    .expect("no poisoned lock")
                    .get(&registry_id)
                    .cloned();
                if let Some(shipper) = shipper {
                    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                    while !shipper.settled() && tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    let current = registries
                        .shippers
                        .lock()
                        .expect("no poisoned lock")
                        .get(&registry_id)
                        .is_some_and(|s| Arc::ptr_eq(s, &shipper));
                    if current {
                        registries
                            .shippers
                            .lock()
                            .expect("no poisoned lock")
                            .remove(&registry_id);
                        registries
                            .ship_states
                            .lock()
                            .expect("no poisoned lock")
                            .remove(&registry_id);
                    }
                }
            });
        }
        Ok(handle)
    }

    async fn route(self: Arc<Self>, mut target: ObjectTarget) -> Result<serde_json::Value, String> {
        // Calls leaving a vm belong to that vm's script; the queue journal
        // records it as the producer. Dashboard dispatches arrive through
        // the internal transport instead and carry their own (absent)
        // caller.
        target.caller = Some(actias_worker_core::extensions::objects::CallerIdentity {
            script: self.prepared.script.public_identifier.clone(),
            revision: self.prepared.revision_id.clone(),
        });
        self.route_inner(target, true)
            .await
            .map_err(RouteError::into_message)
    }

    /// The node holding the object's lease and the lease's epoch, when
    /// the holder is another node; [`None`] when nobody holds it or this
    /// node does. Read from the registry every time: a read served from
    /// a copy is only as correct as the lease it was checked against,
    /// and the holder cache is allowed to be a residency behind.
    async fn live_holder(&self, object_id: &str) -> Option<(String, u64)> {
        let own = self
            .state
            .node_identity
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let lease = self
            .state
            .registry
            .clone()
            .get_lease(actias_worker_core::proto::node_registry::GetLeaseRequest {
                object_id: object_id.to_owned(),
            })
            .await
            .ok()?
            .into_inner();
        self.state
            .holders
            .insert(object_id.to_owned(), lease.node_id.clone())
            .await;
        (own.as_deref() != Some(lease.node_id.as_str())).then_some((lease.node_id, lease.epoch))
    }

    /// A read from this node's replica copy, once the owner has vouched
    /// for it: the owner's watermark names the generation and WAL length
    /// its callers have been told is durable, and the copy is served only
    /// after reaching it. [`None`] when this node holds no copy, the
    /// owner did not answer, or the copy did not catch up in time; the
    /// caller forwards the read to the owner instead.
    async fn confirmed_replica_read(
        &self,
        holder: &str,
        lease_epoch: u64,
        object_id: &str,
        target: &ObjectTarget,
    ) -> Option<serde_json::Value> {
        let (epoch, base, _) = self.state.replica_store.held_generation(object_id).await?;
        let address = crate::directory::route::address_of(&self.state, holder)
            .await
            .ok()?;
        let mut client = crate::data_plane::peer_client(&self.state, &address)
            .await
            .ok()?;
        let watermark = tokio::time::timeout(
            self.state.replica_ack,
            client.watermark(crate::data_plane::authed(
                &self.state.internal_token,
                actias_worker_core::proto::worker_data::WatermarkQuery {
                    object_id: object_id.to_owned(),
                },
            )),
        )
        .await
        .ok()?
        .ok()?
        .into_inner();
        // The owner must hold the object under the lease the registry
        // names, must have released a flight in this residency, and the
        // copy must be of that flight's generation: an owner that lost
        // the lease without knowing, or a copy from an older residency,
        // vouches for nothing.
        if !watermark.held
            || !watermark.released
            || watermark.epoch != lease_epoch
            || (watermark.epoch, watermark.base) != (epoch, base)
        {
            return None;
        }
        let already = self
            .state
            .replica_store
            .held_generation(object_id)
            .await
            .is_some_and(|(_, _, len)| len >= watermark.length);
        if !already {
            let reached = self
                .state
                .replica_store
                .wait_for(object_id, epoch, base, watermark.length, REPLICA_READ_WAIT)
                .await;
            if !reached {
                return None;
            }
            self.state
                .replica_store
                .gauges
                .reads_waited
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let copy = self.state.replica_store.read_copy(object_id).await.ok()??;
        read_bypass(&copy, target).await.ok()?
    }

    /// One hop to the lease holder's data plane; its answer is the answer.
    /// Ends a live residency wherever it is, straight at the lease
    /// holder: the identity is tombstoned by the time this runs, so the
    /// claim path would refuse instead of forwarding. Cold (unheld)
    /// needs nothing. Best effort; the marker heals whatever this
    /// misses.
    ///
    /// # Errors
    /// Returns the holder's status message. A home that is already gone is
    /// not an error.
    pub async fn end_residency(&self, key: &ObjectKey) -> Result<(), String> {
        let lease = match self
            .state
            .registry
            .clone()
            .get_lease(actias_worker_core::proto::node_registry::GetLeaseRequest {
                object_id: key.object_id(),
            })
            .await
        {
            Ok(lease) => lease.into_inner(),
            Err(status) if status.code() == tonic::Code::NotFound => return Ok(()),
            Err(status) => return Err(status.to_string()),
        };

        let target = ObjectTarget {
            class: key.class().to_owned(),
            name: key.name().to_owned(),
            method: "__destroy".to_owned(),
            arguments: Vec::new(),
            chain: Vec::new(),
            caller: None,
        };
        let own = self
            .state
            .node_identity
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        if own.as_deref() == Some(lease.node_id.as_str()) {
            let Some(handle) = self
                .state
                .objects
                .handle_if_resident(&key.to_string())
                .await
            else {
                return Ok(());
            };
            handle
                .call(
                    "__dispatch",
                    serde_json::json!({
                        "class": target.class,
                        "name": target.name,
                        "method": "__destroy",
                        "args": [],
                        "chain": [],
                    }),
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            self.forward(&lease.node_id, key, &target, Vec::new())
                .await
                .map(|_| ())
                .map_err(|error| match error {
                    ForwardError::StaleHome(text) | ForwardError::Call(text) => text,
                })
        }
    }

    async fn forward(
        &self,
        holder: &str,
        key: &ObjectKey,
        target: &ObjectTarget,
        chain: Vec<String>,
    ) -> Result<serde_json::Value, ForwardError> {
        // A node id's address never changes, so the cache spares the
        // registry a read per forward; a gone id reads as a stale home.
        let address = match self.state.node_addrs.get(holder).await {
            Some(address) => address,
            None => {
                let node = self
                    .state
                    .registry
                    .clone()
                    .get_node(actias_worker_core::proto::node_registry::GetNodeRequest {
                        node_id: holder.to_owned(),
                    })
                    .await
                    .map_err(|e| match e.code() {
                        tonic::Code::NotFound => ForwardError::StaleHome(format!(
                            "The object's home is gone: {}",
                            e.message()
                        )),
                        _ => ForwardError::Call(format!(
                            "The object's home could not be resolved: {e}"
                        )),
                    })?
                    .into_inner();
                self.state
                    .node_addrs
                    .insert(holder.to_owned(), node.address.clone())
                    .await;
                node.address
            }
        };

        let mut client = crate::data_plane::peer_client(&self.state, &address)
            .await
            .map_err(ForwardError::StaleHome)?;
        let call = actias_worker_core::proto::worker_data::ObjectCall {
            scope_id: key.scope().to_owned(),
            class: target.class.clone(),
            name: target.name.clone(),
            method: target.method.clone(),
            arguments_json: serde_json::Value::Array(target.arguments.clone()).to_string(),
            // The chain already includes the target; the receiver
            // dispatches without extending again.
            chain,
            // The true producer survives the hop; the receiver must not
            // substitute the owner it resolves.
            caller: target.caller.as_ref().map(|caller| {
                actias_worker_core::proto::worker_data::Caller {
                    script: caller.script.clone(),
                    revision: caller.revision.clone(),
                }
            }),
            // This hop is the one a first hop is allowed; spent now.
            first_hop: false,
        };

        let result = client
            .dispatch(crate::data_plane::authed(&self.state.internal_token, call))
            .await
            .map_err(|e| {
                ForwardError::StaleHome(format!(
                    "The object's home did not answer: {}",
                    e.message()
                ))
            })?
            .into_inner();

        if result.wrong_home {
            return Err(ForwardError::StaleHome(result.error));
        }
        if !result.error.is_empty() {
            return Err(ForwardError::Call(result.error));
        }
        serde_json::from_str(&result.result_json)
            .map_err(|e| ForwardError::Call(format!("The object's home answered garbage: {e}")))
    }

    /// The routing body; `allow_forward` is false for calls that already
    /// arrived over the internal transport, so a stale lease view can
    /// never bounce a call between nodes.
    pub async fn route_inner(
        self: Arc<Self>,
        target: ObjectTarget,
        allow_forward: bool,
    ) -> Result<serde_json::Value, RouteError> {
        let key = ObjectKey::scoped(
            &self.prepared.script.project_id,
            &self.prepared.script.id,
            &target.class,
            &target.name,
        );
        let key_string = key.to_string();

        // Reads that tolerate bounded staleness skip the mailbox entirely.
        // A resident object's own file is the freshest copy there is; a
        // non-resident one reads from a snapshot replica restored beside
        // it, refreshed past its ttl, which is the whole multi-node read
        // story: reads never need the home. A database nothing has shipped
        // yet falls through, so first touch still creates and migrates it
        // through the owner.
        // A request reads its own writes: an object it already called
        // through the mailbox is read there too, never from a copy.
        // A forwarded call arrives with its chain already extended through
        // this target; extending again would refuse it as its own cycle.
        let chain = if target.chain.last().map(String::as_str) == Some(key_string.as_str()) {
            target.chain.clone()
        } else {
            actias_worker_core::objects::extend_call_chain(&target.chain, &key_string)
                .map_err(RouteError::Failed)?
        };

        let called_it = self
            .deferred
            .as_ref()
            .is_some_and(|gates| gates.called(&key_string));
        if target.class == actias_worker_core::extensions::objects::DATABASE_CLASS
            && matches!(target.method.as_str(), "read" | "read_one")
            && !called_it
        {
            let object_id = key.object_id();
            let file = self.state.object_data_dir.join(key.db_file_name());

            if self.state.objects.is_resident(&key_string).await && file.exists() {
                if let Some(result) = read_bypass(&file, &target)
                    .await
                    .map_err(RouteError::Failed)?
                {
                    return Ok(result);
                }
            } else if allow_forward
                && let Some((holder, lease_epoch)) = self.live_holder(&object_id).await
            {
                // The owner lives elsewhere. This node's own copy serves
                // the read once the owner has vouched for it: the
                // watermark is what callers were told is durable, and the
                // append carrying it is already on its way here. Otherwise
                // the owner answers the read itself, from its live file.
                let gauges = self.state.replica_store.gauges.clone();
                if let Some(result) = self
                    .confirmed_replica_read(&holder, lease_epoch, &object_id, &target)
                    .await
                {
                    gauges
                        .reads_confirmed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.state
                        .metrics
                        .replica_reads
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(result);
                }
                gauges
                    .reads_forwarded
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match self.forward(&holder, &key, &target, chain.clone()).await {
                    Ok(value) => return Ok(value),
                    Err(ForwardError::Call(error)) => return Err(RouteError::Failed(error)),
                    // A stale home: the object is cold or moved, and the
                    // ordinary path below wakes it where it belongs.
                    Err(ForwardError::StaleHome(_)) => {}
                }
            }
            // Cold, or held here without a file yet: the ordinary path
            // wakes the object and reads its live file.
        }

        // Past the bypass, this call reaches the object's mailbox, local
        // or forwarded: the request's later reads of it go there too.
        if let Some(gates) = &self.deferred {
            gates.note_call(&key_string);
        }

        // First pass trusts the holder cache; a stale-home answer
        // invalidates it and asks the placement store once for the truth.
        // A call that already crossed the transport skips the cache:
        // the receiver answers from the registry's truth, so two stale
        // caches can never bounce a call between nodes.
        let mut cached_pass = allow_forward;
        let handle = loop {
            match self.resolve_handle(&key, cached_pass).await {
                Ok(handle) => break handle,
                // The incumbent lives: the call belongs on its node, one hop.
                Err(ResolveError::Elsewhere(holder)) if allow_forward => {
                    match self.forward(&holder, &key, &target, chain.clone()).await {
                        Ok(value) => return Ok(value),
                        Err(ForwardError::StaleHome(reason)) if cached_pass => {
                            actias_common::tracing::debug!(
                                reason,
                                holder,
                                "stale home; re-resolving through the registry"
                            );
                            self.state.holders.invalidate(&key.object_id()).await;
                            self.state.node_addrs.invalidate(&holder).await;
                            cached_pass = false;
                            continue;
                        }
                        Err(ForwardError::StaleHome(reason)) => {
                            return Err(RouteError::Failed(reason));
                        }
                        Err(ForwardError::Call(error)) => {
                            return Err(RouteError::Failed(error));
                        }
                    }
                }
                Err(ResolveError::Elsewhere(holder)) => {
                    return Err(RouteError::WrongHome { holder });
                }
                Err(ResolveError::Other(error)) => return Err(RouteError::Failed(error)),
            }
        };

        let payload = serde_json::json!({
            "class": target.class,
            "name": target.name,
            "method": target.method,
            "args": target.arguments,
            "chain": chain,
            "caller": target.caller.as_ref().map(|caller| serde_json::json!({
                "script": caller.script,
                "revision": caller.revision,
            })),
        });
        match &self.deferred {
            // A request's call answers at commit; the gate joins the
            // request's, settled once before anything leaves.
            Some(gates) => {
                let (value, gate) = handle
                    .call_deferred("__dispatch", payload)
                    .await
                    .map_err(|e| RouteError::Failed(e.to_string()))?;
                if let Some(gate) = gate {
                    gates.push(gate);
                }
                Ok(value)
            }
            None => handle
                .call("__dispatch", payload)
                .await
                .map_err(|e| RouteError::Failed(e.to_string())),
        }
    }
}

/// The registry mirror for one object's alarm: `Some(due_ms)` upserts the
/// row, [`None`] deletes it, each write in its own task with a short
/// retry, off every call's transaction, so arming an alarm never pays a
/// postgres round trip. Exhausted retries are logged and tolerated: the
/// local file still holds the alarm, and the spawn-time sync re-mirrors
/// on the next residency. Two rapid writes may land out of order; the
/// stale row that leaves costs one wasted wake and heals the same way.
fn alarm_mirror(
    state: &AppState,
    object_id: &str,
    own_key: &str,
) -> actias_worker_core::objects::AlarmSync {
    let registry = state.registry.clone();
    let object_id = object_id.to_owned();
    let own_key = own_key.to_owned();

    Arc::new(move |due_ms| {
        let mut registry = registry.clone();
        let object_id = object_id.clone();
        let own_key = own_key.clone();

        tokio::spawn(async move {
            const ATTEMPTS: u32 = 3;
            for attempt in 0..ATTEMPTS {
                let written = match due_ms {
                    Some(due_ms) => registry
                        .set_alarm(actias_worker_core::proto::node_registry::SetAlarmRequest {
                            object_id: object_id.clone(),
                            own_key: own_key.clone(),
                            due_ms,
                        })
                        .await
                        .map(|_| ()),
                    None => registry
                        .clear_alarm(
                            actias_worker_core::proto::node_registry::ClearAlarmRequest {
                                object_id: object_id.clone(),
                            },
                        )
                        .await
                        .map(|_| ()),
                };
                match written {
                    Ok(()) => return,
                    Err(_) if attempt + 1 < ATTEMPTS => {
                        tokio::time::sleep(std::time::Duration::from_millis(250 << attempt)).await;
                    }
                    Err(status) => {
                        actias_common::tracing::warn!(
                            error = %status,
                            object_id,
                            "alarm mirror write failed; the boot scan heals it"
                        );
                    }
                }
            }
        });
    })
}

/// The epoch this node last held the object under, from the sidecar
/// beside its file; 0 when unknown, which errs toward restoring.
fn read_resident_epoch(file: &std::path::Path) -> u64 {
    std::fs::read_to_string(file.with_extension("epoch"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Records the residency epoch beside the file. Best effort: a lost
/// sidecar reads as 0 next time and only costs one extra restore.
fn write_resident_epoch(file: &std::path::Path, epoch: u64) {
    if let Err(error) = std::fs::write(file.with_extension("epoch"), epoch.to_string()) {
        actias_common::tracing::warn!(%error, "resident epoch sidecar write failed");
    }
}

/// Longest a replica read waits for the append carrying the owner's
/// watermark, which is already in flight when the wait begins.
const REPLICA_READ_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

/// The replica file for one object, restored from the last shipped
/// snapshot and reused until it ages past the ttl. [`None`] when nothing
/// was ever shipped. Serves the console's stats read on a node holding
/// no copy, so it never depends on where the object is homed.
pub(crate) async fn fresh_replica_file(
    state: &AppState,
    object_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let dir = state.object_data_dir.join("replicas");
    let replica = dir.join(format!("{object_id}.db"));

    let fresh = std::fs::metadata(&replica)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < state.replica_ttl);

    if !fresh {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        if !state.object_store.restore(object_id, &replica).await? {
            return Ok(None);
        }
    }

    Ok(Some(replica))
}

/// One bypassed read: a fresh read-only connection, the query, done.
/// Returns [`None`] when the arguments do not fit a read, letting the
/// mailbox path produce its usual error shapes.
async fn read_bypass(
    file: &std::path::Path,
    target: &ObjectTarget,
) -> Result<Option<serde_json::Value>, String> {
    let Some(serde_json::Value::String(sql)) = target.arguments.first().cloned() else {
        return Ok(None);
    };
    let params: Vec<serde_json::Value> = match target.arguments.get(1) {
        Some(serde_json::Value::Array(params)) => params.clone(),
        Some(serde_json::Value::Null) | None => Vec::new(),
        Some(_) => return Ok(None),
    };
    let one = target.method == "read_one";

    let file = file.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut reader = actias_worker_core::storage::SqliteStorage::open_read_only(&file)?;
        let rows = reader.query(&sql, &params)?;

        Ok(Some(if one {
            rows.into_iter().next().unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Array(rows)
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::{read_resident_epoch, write_resident_epoch};

    #[test]
    fn the_sidecar_round_trips_and_absence_reads_as_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("abc123.db");

        assert_eq!(read_resident_epoch(&file), 0);

        write_resident_epoch(&file, 7);
        assert_eq!(read_resident_epoch(&file), 7);

        // A corrupt sidecar errs toward restoring, never toward serving
        // a possibly stale file.
        std::fs::write(file.with_extension("epoch"), "not a number").expect("write");
        assert_eq!(read_resident_epoch(&file), 0);
    }
}
