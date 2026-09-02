//! An object's home on this node: its storage, alarm, snapshot and
//! placement claim, shared between the task and the host.

use super::*;

/// Runs the platform's deletion sequence for this object after a call
/// that asked to be its last: tombstone, store marker, local files,
/// purge. Owned by the host, which knows the identity and the stores;
/// a failure is logged and the janitor finishes from the tombstone.
pub type DestroyFn = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;

/// Restates the holder's claim, refreshing the declared lifespan: a
/// busy object never ends its residency, so without this the expiry
/// sweep would read a hot object as abandoned. Fire-and-forget and
/// throttled by the task; the host supplies the re-claim.
pub type KeepClaimed = Arc<dyn Fn() + Send + Sync>;

/// How often a warm object with a lifespan restates its claim.
pub(super) const RESIDENCY_REFRESH_MS: i64 = 10 * 60 * 1000;

/// Mirrors this object's armed alarm into an external registry:
/// `Some(due_ms)` on arm, [`None`] on clear. One closure per object with
/// the identity baked in, so nothing guest- or identity-shaped leaks in
/// here. Fire-and-forget by contract: the mirror rides off the call's
/// transaction, a spurious row only ever costs a wasted wake, and the
/// dangerous direction (a missing row) is healed by the spawn-time sync.
pub type AlarmSync = Arc<dyn Fn(Option<i64>) + Send + Sync>;

/// Everything the pinned task owns about its object, in one place: the
/// task is the owner, and the vm holds a clone of the [`Arc`] as app data
/// so the Lua extension surface (`state.sql`, `state:set_alarm`) reaches
/// the same cells. Platform classes take it directly, which is what keeps
/// them free of any guest runtime type.
///
/// The locks are never contended: the mailbox serializes every consumer
/// by construction, so each lock is take-use-release on one task.
pub struct ObjectHome {
    pub(super) storage: Option<std::sync::Mutex<crate::storage::SqliteStorage>>,
    pub(super) alarm: std::sync::Mutex<Option<crate::extensions::objects::PendingAlarm>>,
    pub(super) ship_mark: std::sync::atomic::AtomicI64,
    pub(super) migrations_checked: std::sync::atomic::AtomicBool,
    pub(super) queue_policy: crate::platform::queue::QueuePolicy,
    pub(super) revision: Option<Arc<crate::runtime::PreparedRevision>>,
    /// The registry mirror, when the host wired one; invoked wherever the
    /// alarm cells change.
    pub(super) alarm_sync: Option<AlarmSync>,
    /// The stream delivery timer: earliest moment any edge has work.
    /// Local to residency (edges are durable; this is not), it blocks
    /// hibernation like a pending alarm does.
    pub(super) delivery_due: std::sync::Mutex<Option<i64>>,
    /// This object's identity, learned at first publish; what delivered
    /// events carry as `from`.
    publisher: std::sync::Mutex<Option<(String, String)>>,
    /// Set by `state:destroy()`; the task reads it after the call's
    /// commit and answer, so destruction is the last thing that runs.
    pub(super) destroy_requested: std::sync::atomic::AtomicBool,
    /// When the claim was last restated; the residency refresh throttle.
    pub(super) claim_refreshed_ms: std::sync::atomic::AtomicI64,
}

impl ObjectHome {
    /// A home over a throwaway restored copy, for deriving a row from
    /// an object that must not be woken.
    ///
    /// Everything that could reach the live object is simply absent
    /// rather than suppressed: no pending alarm (arming one here would
    /// fire a real handler against a copy that is about to be deleted,
    /// which is the hazard scratch evaluation exists to avoid), no
    /// alarm mirror, no queue delivery of consequence. The caller owns
    /// the file and deletes it.
    pub fn for_scratch(
        storage: crate::storage::SqliteStorage,
        revision: Option<Arc<crate::runtime::PreparedRevision>>,
    ) -> Self {
        Self::new(
            Some(storage),
            None,
            crate::platform::queue::QueuePolicy::default(),
            revision,
            None,
        )
    }

    pub(super) fn new(
        storage: Option<crate::storage::SqliteStorage>,
        pending: Option<crate::extensions::objects::PendingAlarm>,
        queue_policy: crate::platform::queue::QueuePolicy,
        revision: Option<Arc<crate::runtime::PreparedRevision>>,
        alarm_sync: Option<AlarmSync>,
    ) -> Self {
        Self {
            storage: storage.map(std::sync::Mutex::new),
            alarm: std::sync::Mutex::new(pending),
            ship_mark: std::sync::atomic::AtomicI64::new(0),
            migrations_checked: std::sync::atomic::AtomicBool::new(false),
            queue_policy,
            revision,
            alarm_sync,
            delivery_due: std::sync::Mutex::new(None),
            publisher: std::sync::Mutex::new(None),
            destroy_requested: std::sync::atomic::AtomicBool::new(false),
            claim_refreshed_ms: std::sync::atomic::AtomicI64::new(
                crate::extensions::objects::unix_now_ms(),
            ),
        }
    }

    /// Whether the residency refresh is due; advancing the throttle and
    /// answering in one step, so at most one refresh fires per window.
    pub(super) fn take_refresh_due(&self) -> bool {
        let now = crate::extensions::objects::unix_now_ms();
        let last = self
            .claim_refreshed_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if now - last < RESIDENCY_REFRESH_MS {
            return false;
        }
        self.claim_refreshed_ms
            .compare_exchange(
                last,
                now,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
    }

    /// `state:destroy()`: forget this object once the current call has
    /// committed and answered.
    pub fn request_destroy(&self) {
        self.destroy_requested
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn destroy_requested(&self) -> bool {
        self.destroy_requested
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Marks this object as having published: records its identity for
    /// `from` stamps and wakes the delivery pump now.
    pub fn note_publisher(&self, class: String, name: String) {
        *self.publisher.lock().expect("no poisoned lock") = Some((class, name));
        self.set_delivery_due(Some(crate::extensions::objects::unix_now_ms()));
    }

    /// The publishing identity, when one has published this residency.
    pub fn publisher_identity(&self) -> Option<(String, String)> {
        self.publisher.lock().expect("no poisoned lock").clone()
    }

    pub fn set_delivery_due(&self, due: Option<i64>) {
        let mut slot = self.delivery_due.lock().expect("no poisoned lock");
        *slot = match (*slot, due) {
            (Some(held), Some(new)) => Some(held.min(new)),
            (held, new) => new.or(held),
        };
    }

    /// Clears and returns the delivery timer; the pump re-arms what
    /// remains.
    pub fn take_delivery_due(&self) -> Option<i64> {
        self.delivery_due.lock().expect("no poisoned lock").take()
    }

    pub fn delivery_due(&self) -> Option<i64> {
        *self.delivery_due.lock().expect("no poisoned lock")
    }

    /// Tells the registry mirror what the alarm cell now holds.
    pub(super) fn mirror_alarm(&self, due_ms: Option<i64>) {
        if let Some(sync) = &self.alarm_sync {
            sync(due_ms);
        }
    }

    /// Whether the object has a durable half at all.
    pub fn has_storage(&self) -> bool {
        self.storage.is_some()
    }

    /// Runs one operation against the object's storage; the lock never
    /// outlives the closure, so callers are free to await between
    /// operations.
    ///
    /// # Errors
    /// Returns the operation's error, or a message when the object has no
    /// durable storage at all.
    pub fn with_storage<T>(
        &self,
        operation: impl FnOnce(&mut crate::storage::SqliteStorage) -> Result<T, String>,
    ) -> Result<T, String> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| "This object has no durable storage.".to_owned())?;
        operation(&mut lock_unpoisoned(storage))
    }

    /// Arms the object's one alarm; setting replaces. The persisted row
    /// rides the current call's transaction, the in-memory cell wakes the
    /// task loop; this is the only place both homes are written.
    ///
    /// # Errors
    /// Returns SQLite's message when the persisted row cannot be written.
    pub fn set_alarm(&self, alarm: crate::extensions::objects::PendingAlarm) -> Result<(), String> {
        if self.has_storage() {
            self.with_storage(|storage| {
                storage.save_alarm(alarm.due_ms, &alarm.class, &alarm.name, &alarm.own_key)
            })?;
        }
        self.mirror_alarm(Some(alarm.due_ms));
        *lock_unpoisoned(&self.alarm) = Some(alarm);
        Ok(())
    }

    /// The alarm currently armed, if any.
    pub fn pending_alarm(&self) -> Option<crate::extensions::objects::PendingAlarm> {
        lock_unpoisoned(&self.alarm).clone()
    }

    /// Drops the alarm from both homes; called the moment it fires, so a
    /// handler that sets a new one is not clobbered afterwards.
    pub(super) fn clear_alarm(&self) {
        *lock_unpoisoned(&self.alarm) = None;
        self.mirror_alarm(None);
        if self.has_storage()
            && let Err(error) = self.with_storage(|storage| storage.clear_alarm())
        {
            actias_common::tracing::warn!(%error, "alarm could not be cleared");
        }
    }

    /// Rereads the alarm cell from the persisted row after a rollback:
    /// the rolled-back row is the truth, and the in-memory alarm must not
    /// outlive an alarm the failed method set.
    pub(super) fn resync_alarm_from_storage(&self) {
        use crate::extensions::objects::PendingAlarm;

        let persisted = self
            .with_storage(|storage| storage.load_alarm())
            .ok()
            .flatten()
            .map(|(due_ms, class, name, own_key)| PendingAlarm {
                due_ms,
                class,
                name,
                own_key,
            });
        // The rolled-back truth replaces whatever the failed call
        // mirrored, arm or clear alike.
        self.mirror_alarm(persisted.as_ref().map(|alarm| alarm.due_ms));
        *lock_unpoisoned(&self.alarm) = persisted;
    }

    /// Whether storage changed since the last shipped snapshot, without
    /// advancing the mark: the directory asks before the commit, and
    /// only the gate's own check may consume it.
    pub(super) fn wrote_since_mark(&self) -> bool {
        use std::sync::atomic::Ordering;

        let current = self
            .with_storage(|storage| storage.total_changes())
            .unwrap_or(0);
        current != self.ship_mark.load(Ordering::Relaxed)
    }

    /// Whether storage changed since the last shipped snapshot, advancing
    /// the mark when it did; only calls that wrote pay the shipping toll.
    pub(super) fn writes_advanced(&self) -> bool {
        use std::sync::atomic::Ordering;

        let current = self
            .with_storage(|storage| storage.total_changes())
            .unwrap_or(0);
        if current == self.ship_mark.load(Ordering::Relaxed) {
            return false;
        }
        self.ship_mark.store(current, Ordering::Relaxed);
        true
    }

    /// Whether pending migrations still need checking this vm life. The
    /// applied table in the file is the durable record; this only skips
    /// re-reading it per call. Marked separately so a failed migration
    /// stays unchecked and retries on the next touch.
    pub(crate) fn migrations_unchecked(&self) -> bool {
        !self
            .migrations_checked
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Notes that migrations were checked and applied for this vm life.
    pub(crate) fn mark_migrations_checked(&self) {
        self.migrations_checked
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Delivery limits for `__queue` instances.
    pub fn queue_policy(&self) -> &crate::platform::queue::QueuePolicy {
        &self.queue_policy
    }

    /// The revision this vm runs; platform classes read migrations from
    /// it without touching the vm.
    pub fn revision(&self) -> Option<&Arc<crate::runtime::PreparedRevision>> {
        self.revision.as_ref()
    }
}

/// A poisoned lock has no observer to protect here (the mailbox already
/// serializes every consumer), so the inner value is recovered rather
/// than panicking a request path.
pub(super) fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
