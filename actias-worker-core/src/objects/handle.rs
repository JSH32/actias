//! The handle a caller holds: a mailbox sender and the gate that
//! decides when an answer may be released.

use super::*;

/// How deep one object's mailbox goes before senders wait; backpressure,
/// never a drop policy.
pub(super) const MAILBOX_DEPTH: usize = 128;

/// One queued method call and the channel its answer travels back on.
pub(super) struct ObjectCall {
    pub(super) method: String,
    pub(super) payload: serde_json::Value,
    pub(super) reply: oneshot::Sender<Reply>,
    /// The caller's span, captured at send: the mailbox hop would
    /// otherwise sever the trace, and every effect inside the object
    /// (kv, sql, publishes) would root its own. A caller with no span
    /// (a sweep, a test) makes the dispatch a root, which is truthful.
    pub(super) span: actias_common::tracing::Span,
    /// The caller settles the output gate itself: the answer comes back
    /// at commit with the gate attached, instead of being held until
    /// the write is durable.
    pub(super) defer_gate: bool,
}

/// What a call answers with: the result, and the gate the answer was
/// not held behind when the caller asked to defer it.
pub(super) struct Reply {
    pub(super) result: Result<serde_json::Value, ObjectError>,
    pub(super) gate: Option<GateFuture>,
}

/// A clonable address for one object's mailbox.
#[derive(Clone)]
pub struct ObjectHandle {
    pub(super) sender: mpsc::Sender<ObjectCall>,
    /// Unix milliseconds of the last call sent through this handle;
    /// what the host reads to pick the idlest resident of a scope.
    pub(super) last_call: Arc<std::sync::atomic::AtomicI64>,
}

impl ObjectHandle {
    /// When this object was last called, unix milliseconds; 0 when never.
    pub fn last_call_ms(&self) -> i64 {
        self.last_call.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn note_call(&self) {
        self.last_call.store(
            crate::extensions::objects::unix_now_ms(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Sends one call and waits for its result; calls from any number of
    /// tasks execute one at a time in arrival order.
    ///
    /// # Errors
    /// Returns [`ObjectError::Call`] when the method fails or is missing,
    /// [`ObjectError::Gone`] when the task no longer runs.
    pub async fn call(
        &self,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, ObjectError> {
        let (reply, response) = oneshot::channel();

        self.note_call();
        self.sender
            .send(ObjectCall {
                method: method.to_owned(),
                payload,
                reply,
                span: actias_common::tracing::Span::current(),
                defer_gate: false,
            })
            .await
            .map_err(|_| ObjectError::Gone)?;

        response.await.map_err(|_| ObjectError::Gone)?.result
    }

    /// Resolves once the object's task has ended, by hibernation or
    /// destruction: the mailbox is closed and nothing will answer it.
    pub async fn ended(&self) {
        self.sender.closed().await;
    }

    /// Like [`Self::call`], answered at commit rather than at
    /// durability: the gate comes back with the result for the caller
    /// to settle before anything it does leaves the machine. A caller
    /// that drops the gate unsettled has an answer the write may not
    /// yet deserve, which is why only the request path, which settles
    /// every gate before its response, asks for this.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub async fn call_deferred(
        &self,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<(serde_json::Value, Option<GateFuture>), ObjectError> {
        let (reply, response) = oneshot::channel();

        self.note_call();
        self.sender
            .send(ObjectCall {
                method: method.to_owned(),
                payload,
                reply,
                span: actias_common::tracing::Span::current(),
                defer_gate: true,
            })
            .await
            .map_err(|_| ObjectError::Gone)?;

        let Reply { result, gate } = response.await.map_err(|_| ObjectError::Gone)?;
        Ok((result?, gate))
    }
}

/// The gates a request has deferred: every object write it made whose
/// answer came back at commit. Settled in one wait before the response
/// or any outbound request leaves, so nothing outside the machine
/// learns of a write before it is durable: one wait for a chain of
/// calls rather than one per hop.
#[derive(Default)]
pub struct PendingGates {
    gates: std::sync::Mutex<Vec<GateFuture>>,
    /// Objects this request has called with anything but a bypassed
    /// read. A later read of one of them in the same request goes to
    /// the owner rather than a replica copy, so a request always reads
    /// its own writes whatever node it landed on.
    written: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl PendingGates {
    pub fn push(&self, gate: GateFuture) {
        self.gates.lock().expect("no poisoned lock").push(gate);
    }

    /// Remembers that this request called `key` through its mailbox.
    pub fn note_call(&self, key: &str) {
        self.written
            .lock()
            .expect("no poisoned lock")
            .insert(key.to_owned());
    }

    /// Whether this request has called `key` through its mailbox, so a
    /// read of it must see those writes.
    pub fn called(&self, key: &str) -> bool {
        self.written.lock().expect("no poisoned lock").contains(key)
    }

    /// Waits for every deferred gate taken so far; later pushes wait
    /// for the next settle.
    ///
    /// # Errors
    /// Returns the first write that could not be confirmed durable: the
    /// caller's outcome is unknown, as a held answer's would have been.
    pub async fn settle(&self) -> Result<(), String> {
        let gates: Vec<GateFuture> =
            std::mem::take(&mut *self.gates.lock().expect("no poisoned lock"));
        if gates.is_empty() {
            return Ok(());
        }
        // One after another is one wait in wall time: every flight these
        // watch is already in the air, and a gate resolves when its
        // flight lands whether or not anyone is polling it yet.
        for gate in gates {
            gate.await?;
        }
        Ok(())
    }
}

/// A pending durability confirmation: resolves once the writes it was
/// taken for are safe off this machine, or with the reason they are not.
pub type GateFuture = std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// The output gate, called once after a call that wrote. Calling it
/// starts the write on its way (the host marks its shipper); the
/// [`GateFuture`] it returns resolves when that write is durable, and the
/// call's answer waits on it. Dropping the future therefore keeps the
/// shipping and skips only the waiting, which is what platform-initiated
/// work with no caller to answer does.
///
/// The error is the reason durability could not be confirmed, for the
/// caller to report as an unknown outcome ([`ObjectError::NotDurable`]).
pub type AfterWrite = Arc<dyn Fn() -> GateFuture + Send + Sync>;

/// The output gate for everything else: runs after a call that wrote
/// nothing, and hands back a wait when the object still has writes in
/// flight from earlier calls, so no reply ever describes state a crash
/// could take back. [`None`] when the object is settled, which is the
/// common case and costs nothing.
pub type AfterRead = Arc<dyn Fn() -> Option<GateFuture> + Send + Sync>;

/// One dispatched call's answer, plus the gate its answer waits behind.
pub(super) struct Dispatched {
    pub(super) result: Result<serde_json::Value, ObjectError>,
    /// Present when the call wrote and its answer is worth gating; the
    /// mailbox awaits it off the task so the next call runs meanwhile.
    pub(super) gate: Option<GateFuture>,
}
