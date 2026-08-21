//! Object call routing: from an identity to the owner's current code to
//! the pinned vm, with forwarding to the lease holder and replica-backed
//! reads. The http surface in [`crate::server`] only decodes transports
//! into these calls; everything about placement and code resolution lives
//! here.

use std::sync::Arc;

use actias_common::logging::script_log_channel;
use actias_worker_core::extensions::log::LogPublisher;
use actias_worker_core::extensions::objects::{ObjectRouter, ObjectTarget};
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
/// CALLER's context (whose project and script derive identities); the
/// code an object runs is always the owner's current revision, resolved
/// per identity through [`owner_prepared`].
pub struct ObjectRouting {
    state: AppState,
    /// The CALLER context: whose project and script derive identities.
    pub(crate) prepared: Arc<PreparedRevision>,
}

/// Per-call budget for one object method, mirroring the request deadline.
const OBJECT_CALL_BUDGET_SECS: u64 = 10;

/// Why an object could not be made resident here.
pub enum ResolveError {
    /// A live incumbent holds the lease; forward the call to it.
    Elsewhere(String),
    Other(String),
}

impl ObjectRouting {
    /// Routing for one prepared revision over this node's shared pieces.
    pub fn new(state: &AppState, prepared: Arc<PreparedRevision>) -> Arc<Self> {
        Arc::new(Self {
            state: state.clone(),
            prepared,
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

    /// The lease claim for one identity, spoken as this node. The claim
    /// carries the key's preimage plus the owner script as directory
    /// metadata.
    async fn claim_lease(
        &self,
        key: &ObjectKey,
        owner_script_id: &str,
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
                script_id: owner_script_id.to_owned(),
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
    ) -> Result<actias_worker_core::objects::ObjectHandle, ResolveError> {
        // The code the object runs is the owner's current revision,
        // whoever is calling; resolved before the lease so the claim can
        // record the owner in the directory.
        let owner = owner_prepared(&self.state, key)
            .await
            .map_err(ResolveError::Other)?;

        // A non-resident object needs the lease before anything spawns;
        // a resident one already holds it (leases live as long as we do).
        if !self.state.objects.is_resident(&key.to_string()).await {
            let lease = self
                .claim_lease(key, &owner.script.id)
                .await
                .map_err(ResolveError::Other)?;
            if !lease.acquired {
                return Err(ResolveError::Elsewhere(lease.node_id));
            }
        }

        self.resolve_local(key, owner)
            .await
            .map_err(ResolveError::Other)
    }

    /// The spawn itself, lease already settled (or re-settled by the
    /// factory for the resident-revision-bump edge, where it is our own).
    async fn resolve_local(
        self: &Arc<Self>,
        key: &ObjectKey,
        owner: Arc<PreparedRevision>,
    ) -> Result<actias_worker_core::objects::ObjectHandle, String> {
        let routing = self.clone();
        // The hash is the object's platform-wide id: the lease key in the
        // placement store and the file name on disk, because the class and
        // instance names are user-chosen text.
        let object_id = key.object_id();
        let file = self.state.object_data_dir.join(key.db_file_name());
        let identity = key.clone();
        let marker = owner.revision_id.clone();

        self.state
            .objects
            .get_or_spawn(&key.to_string(), &marker, || async move {
                // One claim per residency, before anything is built: an
                // object only ever lives where its lease is held, which is
                // what makes a second node refusing to serve it correct.
                let lease = routing
                    .claim_lease(&identity, &owner.script.id)
                    .await
                    .map_err(mlua::Error::RuntimeError)?;
                if !lease.acquired {
                    return Err(mlua::Error::RuntimeError(
                        "Object is homed on another node.".to_owned(),
                    ));
                }

                // No local file means this node has never hosted the
                // object (or lost its volume): the last shipped snapshot
                // is the truth, and restoring it here is rehoming.
                if !file.exists() {
                    match routing.state.object_store.restore(&object_id, &file).await {
                        Ok(true) => {
                            actias_common::tracing::info!(object_id, "object restored from store")
                        }
                        Ok(false) => {}
                        Err(error) => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "The object's snapshot could not be restored: {error}"
                            )));
                        }
                    }
                }

                // A workflow instance replays the revision it STARTED
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
                    // instance file opens BEFORE the vm builds, because
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
                    ActiasRuntime::new(
                        prepared.clone(),
                        routing.state.clients.kv.clone(),
                        routing.state.egress.clone(),
                        logs,
                        routing.state.secret_client.clone(),
                        None,
                    )
                    .await?
                };
                // The pinned vm routes its own outbound calls too; the
                // chain it hands them is what makes cycles refusable. Its
                // routing context matches the code it runs.
                let vm_routing = ObjectRouting::new(&routing.state, prepared);
                runtime.set_app_data::<ObjectRouter>(vm_routing.as_router());

                let mut storage = actias_worker_core::storage::SqliteStorage::open(&file)
                    .map_err(mlua::Error::RuntimeError)?;
                storage
                    .set_size_limit(routing.state.object_db_max_bytes)
                    .map_err(mlua::Error::RuntimeError)?;

                // The output gate: every call that wrote ships its snapshot
                // before the caller hears the result. A refused (fenced) or
                // failed ship is logged; local durability still holds and
                // the next write retries.
                let ship_store = routing.state.object_store.clone();
                let ship_id = object_id.clone();
                let ship_file = file.clone();
                let epoch = lease.epoch;
                let after_write: actias_worker_core::objects::AfterWrite = Arc::new(move || {
                    let store = ship_store.clone();
                    let object_id = ship_id.clone();
                    let file = ship_file.clone();
                    Box::pin(async move {
                        if let Err(error) = store.ship(&object_id, epoch, &file).await {
                            actias_common::tracing::warn!(
                                %error,
                                object_id,
                                "object snapshot did not ship"
                            );
                        }
                    })
                });

                let alarm_sync = alarm_mirror(&routing.state, &object_id, &identity.to_string());

                Ok((
                    runtime,
                    actias_worker_core::objects::TaskOptions {
                        call_budget: Some(OBJECT_CALL_BUDGET_SECS),
                        storage: Some(storage),
                        hibernate_after: Some(routing.state.object_idle_after),
                        after_write: Some(after_write),
                        alarm_sync: Some(alarm_sync),
                        queue: routing.state.queue_policy.clone(),
                    },
                ))
            })
            .await
            .map_err(|e| e.to_string())
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
        self.route_inner(target, true).await
    }

    /// The replica file for one object, [`None`] when nothing was ever
    /// shipped: the caller falls through to the owner.
    async fn fresh_replica(&self, object_id: &str) -> Result<Option<std::path::PathBuf>, String> {
        fresh_replica_file(&self.state, object_id).await
    }

    /// One hop to the lease holder's data plane; its answer is the answer.
    async fn forward(
        &self,
        holder: &str,
        key: &ObjectKey,
        target: &ObjectTarget,
        chain: Vec<String>,
    ) -> Result<serde_json::Value, String> {
        let node = self
            .state
            .registry
            .clone()
            .get_node(actias_worker_core::proto::node_registry::GetNodeRequest {
                node_id: holder.to_owned(),
            })
            .await
            .map_err(|e| format!("The object's home could not be resolved: {e}"))?
            .into_inner();

        let mut client = crate::data_plane::peer_client(&self.state, &node.address).await?;
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
            .map_err(|e| format!("The object's home did not answer: {}", e.message()))?
            .into_inner();

        if !result.error.is_empty() {
            return Err(result.error);
        }
        serde_json::from_str(&result.result_json)
            .map_err(|e| format!("The object's home answered garbage: {e}"))
    }

    /// The routing body; `allow_forward` is false for calls that already
    /// arrived over the internal transport, so a stale lease view can
    /// never bounce a call between nodes.
    pub async fn route_inner(
        self: Arc<Self>,
        target: ObjectTarget,
        allow_forward: bool,
    ) -> Result<serde_json::Value, String> {
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
        if target.class == actias_worker_core::extensions::objects::DATABASE_CLASS
            && matches!(target.method.as_str(), "read" | "read_one")
        {
            let object_id = key.object_id();
            let file = self.state.object_data_dir.join(key.db_file_name());

            if self.state.objects.is_resident(&key_string).await && file.exists() {
                if let Some(result) = read_bypass(&file, &target).await? {
                    return Ok(result);
                }
            } else if let Some(replica) = self.fresh_replica(&object_id).await?
                && let Some(result) = read_bypass(&replica, &target).await?
            {
                self.state
                    .metrics
                    .replica_reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(result);
            }
        }

        // A forwarded call arrives with its chain already extended through
        // this target; extending again would refuse it as its own cycle.
        let chain = if target.chain.last().map(String::as_str) == Some(key_string.as_str()) {
            target.chain.clone()
        } else {
            actias_worker_core::objects::extend_call_chain(&target.chain, &key_string)?
        };
        let handle = match self.resolve_handle(&key).await {
            Ok(handle) => handle,
            // The incumbent lives: the call belongs on its node, one hop.
            Err(ResolveError::Elsewhere(holder)) if allow_forward => {
                return self.forward(&holder, &key, &target, chain).await;
            }
            Err(ResolveError::Elsewhere(holder)) => {
                return Err(format!(
                    "Object is homed on {holder}, but this call may not forward again."
                ));
            }
            Err(ResolveError::Other(error)) => return Err(error),
        };

        handle
            .call(
                "__dispatch",
                serde_json::json!({
                    "class": target.class,
                    "name": target.name,
                    "method": target.method,
                    "args": target.arguments,
                    "chain": chain,
                    "caller": target.caller.as_ref().map(|caller| serde_json::json!({
                        "script": caller.script,
                        "revision": caller.revision,
                    })),
                }),
            )
            .await
            .map_err(|e| e.to_string())
    }
}

/// The registry mirror for one object's alarm: `Some(due_ms)` upserts the
/// row, [`None`] deletes it, each write in its own task with a short
/// retry, OFF every call's transaction, so arming an alarm never pays a
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

/// The replica file for one object, restored from the last shipped
/// snapshot and reused until it ages past the ttl. [`None`] when nothing
/// was ever shipped. Serves the read bypass on non-holders and the stats
/// read, so neither ever depends on where the object is homed.
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
