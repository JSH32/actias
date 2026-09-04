//! The host: every resident object on this node, by id.

use super::*;

/// The registry of live objects on this node, keyed by object identity
/// (never by revision: identity is what storage will hang off).
#[derive(Default)]
pub struct ObjectHost {
    pub(super) tasks: Mutex<HashMap<String, (String, ObjectHandle)>>,
    /// One gate per object mid-spawn, so a cold object's restore holds
    /// up its own callers and nobody else's. An entry lives only while
    /// someone is spawning or waiting to.
    spawning: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The node's resident bound, shared by scope; [`None`] is unbounded.
    residents: Option<Arc<crate::shares::Pool>>,
}

/// The refusal when a scope holds its share of residents and none of
/// them is idle enough to give way.
pub const TOO_MANY_RESIDENTS: &str =
    "This project has too many live objects on this node; retry shortly.";

/// Longest a spawn waits for the resident it evicted to end, and how
/// often it looks.
const EVICTION_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
const EVICTION_POLL: std::time::Duration = std::time::Duration::from_millis(10);

impl ObjectHost {
    /// A host whose residents are bounded by `pool`, split by scope: a
    /// spawn over the scope's share evicts the scope's idlest resident
    /// first, and refuses only when nothing gives way.
    pub fn bounded(pool: Arc<crate::shares::Pool>) -> Self {
        Self {
            residents: Some(pool),
            ..Default::default()
        }
    }

    /// The scope of a registry id (`scope/class/name`); the whole id
    /// when it is not a key, so a bare test id still shares by itself.
    fn scope_of(id: &str) -> &str {
        id.split('/').next().unwrap_or(id)
    }

    /// A residency permit for `id`'s scope, making room by eviction
    /// only when the node is short: a scope over its share gives up its
    /// own idlest object; a scope under its share, refused because the
    /// pool is full, takes the idlest object of whichever scope is
    /// furthest over its share. Nothing is evicted on a node with room.
    ///
    /// # Errors
    /// Returns [`TOO_MANY_RESIDENTS`] when nothing gave way in time.
    async fn residency(&self, id: &str) -> mlua::Result<Option<crate::shares::Permit>> {
        let Some(pool) = &self.residents else {
            return Ok(None);
        };
        let scope = Self::scope_of(id);
        if let Ok(permit) = pool.try_acquire(scope) {
            return Ok(Some(permit));
        }
        let gives_way = if pool.over_share(scope) {
            scope.to_owned()
        } else {
            pool.most_over_share().unwrap_or_else(|| scope.to_owned())
        };
        if self.evict_idlest(&gives_way).await.is_some() {
            // The permit is the task's; it frees when the task ends,
            // which is once the evicted object's last handle is gone.
            // Holding its handle here would be one more, so the wait
            // polls the pool instead of the task.
            let deadline = tokio::time::Instant::now() + EVICTION_WAIT;
            while tokio::time::Instant::now() < deadline {
                if let Ok(permit) = pool.try_acquire(scope) {
                    return Ok(Some(permit));
                }
                tokio::time::sleep(EVICTION_POLL).await;
            }
        }
        pool.try_acquire(scope)
            .map(Some)
            .map_err(|_| mlua::Error::RuntimeError(TOO_MANY_RESIDENTS.to_owned()))
    }

    /// Drops the registry entry of the scope's resident with the oldest
    /// last call; the id it evicted, when there was one.
    async fn evict_idlest(&self, scope: &str) -> Option<String> {
        let prefix = format!("{scope}/");
        let mut tasks = self.tasks.lock().await;
        let idlest = tasks
            .iter()
            .filter(|(id, (_, handle))| id.starts_with(&prefix) && !handle.sender.is_closed())
            .min_by_key(|(_, (_, handle))| handle.last_call_ms())
            .map(|(id, _)| id.clone())?;
        tasks.remove(&idlest);
        Some(idlest)
    }

    /// The handle for `id`, spawning its task on first use. A changed
    /// `marker` (the revision the vm should embody) evicts the old task
    /// and builds a fresh one, so a republish never serves stale code and
    /// retired vms do not accumulate.
    ///
    /// The factory runs under the object's own gate, never the registry
    /// lock: two racing callers can never both build a vm for one object,
    /// and a cold object's restore (a store fetch, a replica takeover)
    /// delays only the callers of that object. The registry lock is held
    /// for lookups and inserts alone.
    ///
    /// # Errors
    /// Returns whatever the factory failed with; nothing is registered.
    pub async fn get_or_spawn<F, Fut>(
        &self,
        id: &str,
        marker: &str,
        factory: F,
    ) -> mlua::Result<ObjectHandle>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = mlua::Result<(ActiasRuntime, TaskOptions)>>,
    {
        if let Some(handle) = self.live(id, marker).await {
            return Ok(handle);
        }
        let gate = {
            let mut spawning = self.spawning.lock().await;
            spawning.entry(id.to_owned()).or_default().clone()
        };
        let spawned = {
            let _spawning = gate.lock().await;
            // Whoever held the gate before us may have spawned it.
            match self.live(id, marker).await {
                Some(handle) => Ok(handle),
                None => match factory().await {
                    Ok((runtime, mut options)) => {
                        // Admission comes after the build so a refused
                        // spawn costs the scope its own vm build only;
                        // the permit rides the task and frees with it.
                        options.residency = match self.residency(id).await {
                            Ok(permit) => permit,
                            Err(error) => {
                                let mut spawning = self.spawning.lock().await;
                                if Arc::strong_count(&gate) <= 2 {
                                    spawning.remove(id);
                                }
                                return Err(error);
                            }
                        };
                        let handle = spawn_object_task(runtime, options);
                        self.tasks
                            .lock()
                            .await
                            .insert(id.to_owned(), (marker.to_owned(), handle.clone()));
                        Ok(handle)
                    }
                    Err(error) => Err(error),
                },
            }
        };
        // The gate leaves with its last user, so the map holds only
        // objects someone is spawning right now.
        let mut spawning = self.spawning.lock().await;
        if Arc::strong_count(&gate) <= 2 {
            spawning.remove(id);
        }
        spawned
    }

    /// The live task for `id` at `marker`, when there is one. A
    /// hibernated task's sender reads closed; it respawns exactly like a
    /// retired revision would.
    async fn live(&self, id: &str, marker: &str) -> Option<ObjectHandle> {
        self.tasks
            .lock()
            .await
            .get(id)
            .filter(|(held, handle)| held == marker && !handle.sender.is_closed())
            .map(|(_, handle)| handle.clone())
    }

    /// How many objects currently have live tasks; hibernated ones do
    /// not count.
    pub async fn resident_count(&self) -> usize {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|(_, handle)| !handle.sender.is_closed())
            .count()
    }

    /// The ids of every object with a live task, for a walk that asks
    /// each whether it still belongs here.
    pub async fn resident_ids(&self) -> Vec<String> {
        self.tasks
            .lock()
            .await
            .iter()
            .filter(|(_, (_, handle))| !handle.sender.is_closed())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether the object currently has a live task; a hibernated one
    /// reads as absent.
    pub async fn is_resident(&self, id: &str) -> bool {
        self.tasks
            .lock()
            .await
            .get(id)
            .is_some_and(|(_, handle)| !handle.sender.is_closed())
    }

    /// Drops an object's registry entry; its task ends once in-flight
    /// callers finish. The next access builds a fresh vm.
    pub async fn evict(&self, id: &str) {
        self.tasks.lock().await.remove(id);
    }

    /// The live task's handle when one exists; never spawns.
    pub async fn handle_if_resident(&self, id: &str) -> Option<ObjectHandle> {
        self.tasks
            .lock()
            .await
            .get(id)
            .filter(|(_, handle)| !handle.sender.is_closed())
            .map(|(_, handle)| handle.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"on "fetch" (function() return { body = "ok" } end)"#;

    /// A scope at its share of residents makes room by evicting its
    /// idlest object; the evicted task's permit frees once it ends.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_scope_at_its_share_evicts_its_idlest_resident() {
        let host = ObjectHost::bounded(crate::shares::Pool::new("residents", 1, 0.0));
        let first = host
            .get_or_spawn("proj/Room/a", "rev", || async {
                Ok((testing::runtime_with(SCRIPT).await, TaskOptions::default()))
            })
            .await
            .expect("spawns");
        assert!(host.is_resident("proj/Room/a").await);
        // Callers hold handles only for a call; a test's clone would
        // keep the evicted task alive past the eviction wait.
        drop(first);

        let second = host
            .get_or_spawn("proj/Room/b", "rev", || async {
                Ok((testing::runtime_with(SCRIPT).await, TaskOptions::default()))
            })
            .await
            .expect("spawns by evicting a");
        assert!(!host.is_resident("proj/Room/a").await, "a gave way");
        assert!(host.is_resident("proj/Room/b").await);
        assert_eq!(host.resident_count().await, 1);

        // Another scope arriving on the full pool is under its share
        // while proj is over it, so proj's idle b gives way to c.
        drop(second);
        let third = host
            .get_or_spawn("other/Room/c", "rev", || async {
                Ok((testing::runtime_with(SCRIPT).await, TaskOptions::default()))
            })
            .await
            .expect("spawns by evicting proj's idlest");
        assert!(
            !host.is_resident("proj/Room/b").await,
            "b gave way to another scope"
        );
        assert!(host.is_resident("other/Room/c").await);
        drop(third);
    }

    /// A node with room evicts nothing: a scope past its share keeps
    /// spawning while the pool has free permits.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_node_with_room_evicts_nothing() {
        let host = ObjectHost::bounded(crate::shares::Pool::new("residents", 5, 0.0));
        let _other = host
            .get_or_spawn("other/Room/x", "rev", || async {
                Ok((testing::runtime_with(SCRIPT).await, TaskOptions::default()))
            })
            .await
            .expect("spawns");
        // proj's share is two of five; it takes a third because the
        // pool has room past the one-permit floor, and nothing of its
        // own was evicted for it.
        let mut held = Vec::new();
        for name in ["a", "b", "c"] {
            held.push(
                host.get_or_spawn(&format!("proj/Room/{name}"), "rev", || async {
                    Ok((testing::runtime_with(SCRIPT).await, TaskOptions::default()))
                })
                .await
                .expect("spawns"),
            );
        }
        assert_eq!(host.resident_count().await, 4);
        for name in ["a", "b", "c"] {
            assert!(
                host.is_resident(&format!("proj/Room/{name}")).await,
                "{name}"
            );
        }
    }
}
