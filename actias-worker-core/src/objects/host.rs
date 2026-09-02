//! The host: every resident object on this node, by id.

use super::*;

/// The registry of live objects on this node, keyed by object identity
/// (never by revision: identity is what storage will hang off).
#[derive(Default)]
pub struct ObjectHost {
    pub(super) tasks: Mutex<HashMap<String, (String, ObjectHandle)>>,
}

impl ObjectHost {
    /// The handle for `id`, spawning its task on first use. A changed
    /// `marker` (the revision the vm should embody) evicts the old task
    /// and builds a fresh one, so a republish never serves stale code and
    /// retired vms do not accumulate.
    ///
    /// The factory runs under the registry lock, so two racing callers can
    /// never both build a vm for one object; correctness first, and object
    /// construction is rare next to calls.
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
        let mut tasks = self.tasks.lock().await;

        // A hibernated task's sender reads closed; it respawns exactly
        // like a retired revision would.
        if let Some((held, handle)) = tasks.get(id)
            && held == marker
            && !handle.sender.is_closed()
        {
            return Ok(handle.clone());
        }

        let (runtime, options) = factory().await?;
        let handle = spawn_object_task(runtime, options);
        tasks.insert(id.to_owned(), (marker.to_owned(), handle.clone()));

        Ok(handle)
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
