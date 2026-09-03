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
}

impl ObjectHost {
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
                    Ok((runtime, options)) => {
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
