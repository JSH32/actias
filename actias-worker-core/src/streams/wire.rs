//! What travels between nodes for a delivery, and the seams the worker
//! fills in to send it.

/// This runtime's node identity, set by the host so the pump can tell
/// its own connections from another node's. Absent or empty means
/// "treat every edge as local", which is the single-node truth.
#[derive(Clone)]
pub struct LocalNode(pub String);

/// One remote connection edge riding a node batch: which connection,
/// what it follows, how far it has heard, and its filter. The events
/// themselves ride once per node beside these.
pub struct InboxEdge {
    pub edge_id: i64,
    pub connection: String,
    pub topic: String,
    /// Events at or below this seq were already heard.
    pub after: i64,
    pub filter: Option<serde_json::Value>,
}

/// One node's connection fan-out: the due events once, each
/// {seq, topic, from_class, from_name, data}, and every edge they are
/// due for. The receiving node slices per edge by topic, seq and
/// filter, so the payload never multiplies by listeners.
pub struct NodeInbox {
    pub events: Vec<serde_json::Value>,
    pub edges: Vec<InboxEdge>,
}

/// Sends one node's batch and returns the connection ids that node
/// reported gone, so their edges can be pruned. Provided by the host;
/// a runtime without one treats remote edges as unreachable (events
/// missed, edges kept), which at-most-once permits.
pub type ConnectionForwarder = std::sync::Arc<
    dyn Fn(
            String,
            NodeInbox,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<String>, String>> + Send>,
        > + Send
        + Sync,
>;

/// Who this vm publishes as, set by the host at vm creation; batched
/// durable delivery names the publisher so the receiving node can
/// read ranges from the nearest copy of its log.
#[derive(Clone)]
pub struct PublisherIdentity {
    pub scope: String,
    pub class: String,
    pub name: String,
}

/// One durable follower's due events, batched per node. Small sets
/// ride inline; past the cap only the range travels and the receiving
/// node reads the events from the nearest copy of the publisher's log
/// (its own replica, usually).
pub struct ReceiveDelivery {
    pub edge_id: i64,
    pub follower_class: String,
    pub follower_name: String,
    pub topic: String,
    pub filter: Option<serde_json::Value>,
    /// Inline events, each {seq, topic, from_class, from_name, data};
    /// empty when a range travels instead.
    pub events: Vec<serde_json::Value>,
    /// (after, upto]: set when the payload outgrew the inline cap.
    pub range: Option<(i64, i64)>,
}

/// What one node reports back per follower it delivered for.
pub struct ReceiveReport {
    pub follower_class: String,
    pub follower_name: String,
    pub delivered_to: i64,
    pub failed: bool,
}

/// Sends one node's durable batch; the reports drive cursor advances
/// and failure backoff exactly as per-edge delivery would have.
/// Why a per-node batch never reached its node.
#[derive(Debug)]
pub enum ForwardError {
    /// The recorded node id no longer exists in the registry: it will
    /// never come back (a restarted worker registers a fresh id), so
    /// the stale home is cleared and delivery falls back to routing by
    /// the follower's identity, which finds wherever it lives now.
    NodeGone,
    /// The node exists but the call failed; ordinary backoff applies.
    Transport(String),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardError::NodeGone => write!(f, "the follower's node is gone"),
            ForwardError::Transport(error) => write!(f, "{error}"),
        }
    }
}

pub type ReceiveForwarder = std::sync::Arc<
    dyn Fn(
            String,
            PublisherIdentity,
            Vec<ReceiveDelivery>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<ReceiveReport>, ForwardError>> + Send>,
        > + Send
        + Sync,
>;

/// Inline events past this many serialized bytes travel as a range
/// instead, and the receiving node reads them from the nearest copy.
pub(super) const INLINE_EVENT_CAP: usize = 32 * 1024;
