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
    pub(super) reply: oneshot::Sender<Result<serde_json::Value, ObjectError>>,
    /// The caller's span, captured at send: the mailbox hop would
    /// otherwise sever the trace, and every effect inside the object
    /// (kv, sql, publishes) would root its own. A caller with no span
    /// (a sweep, a test) makes the dispatch a root, which is truthful.
    pub(super) span: actias_common::tracing::Span,
}

/// A clonable address for one object's mailbox.
#[derive(Clone)]
pub struct ObjectHandle {
    pub(super) sender: mpsc::Sender<ObjectCall>,
}

impl ObjectHandle {
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

        self.sender
            .send(ObjectCall {
                method: method.to_owned(),
                payload,
                reply,
                span: actias_common::tracing::Span::current(),
            })
            .await
            .map_err(|_| ObjectError::Gone)?;

        response.await.map_err(|_| ObjectError::Gone)?
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

/// One dispatched call's answer, plus the gate its answer waits behind.
pub(super) struct Dispatched {
    pub(super) result: Result<serde_json::Value, ObjectError>,
    /// Present when the call wrote and its answer is worth gating; the
    /// mailbox awaits it off the task so the next call runs meanwhile.
    pub(super) gate: Option<GateFuture>,
}
