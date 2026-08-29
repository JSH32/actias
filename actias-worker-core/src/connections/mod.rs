//! Connections as followers: the transport-neutral primitives every
//! socket rides, the inbox and the registry, plus the actor that
//! drives a declared program over them.
//!
//! # The model
//!
//! A websocket upgrade happens INSIDE fetch: the handler authenticates
//! like any request, then returns `request:upgrade(Class, seed?,
//! identity)` naming a declared connection class. The request's vm is
//! released like any response; the class's handlers run in
//! [`actor::ConnectionTask`], one invocation per inbox item. The
//! connection SPEAKS AS the minted identity; `transport =
//! "connection"` decides only the edge kind. No stdlib connection
//! program exists, and the app's own frames are the only ones the
//! wire carries in either direction, with one exception a class asks
//! for by name: `event = "forward"` sends deliveries in the platform
//! envelope, which is a declared policy rather than a blessed
//! closure.
//!
//! One bounded inbox per connection merges both producers, edge
//! deliveries and the client's own frames; a connection whose handlers
//! cannot keep up fills it, and past the bound the connection closes
//! rather than buffering unboundedly. The registry maps node-local
//! connection ids to inbox senders; a missing id reports Gone, which
//! is a pump's signal to prune the edge (deliver-or-prune,
//! at-most-once, expected-stale after failover). Nothing here adds
//! transport: local delivery is a registry push. Still open:
//! cross-node connection delivery (the edge would record its
//! terminating node and ride the worker dispatch channel; an
//! unroutable node is Gone).

pub mod actor;

use std::collections::HashMap;
use std::sync::Mutex;

/// One connection's inbox capacity. Past this bound the connection is
/// closed rather than buffered: a program that stops pulling has
/// chosen to fall behind, and connection edges promise at-most-once.
const INBOX_BOUND: usize = 256;

/// What the actor pulls, merged from both producers: edge deliveries
/// and the client's own frames.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxItem {
    /// A delivered edge event: topic, publishing instance, data.
    Event {
        topic: String,
        from_class: String,
        from_name: String,
        data: serde_json::Value,
    },
    /// A frame the client sent up the wire, already json-decoded.
    Frame(serde_json::Value),
    /// The wire closed; the program's loop ends after draining.
    Closed,
}

/// What a connection program sends toward the wire; the worker's
/// bridge encodes these as websocket frames. Channel-level data on
/// purpose: no ws types cross into worker-core.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboundFrame {
    /// One json frame down the wire.
    Json(serde_json::Value),
    /// Close the wire politely.
    Close,
}

/// Why a delivery did not land; both mean "prune the edge" to a pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRefused {
    /// No such connection here: it closed, or died with a node.
    Gone,
    /// The inbox is full: the program stopped pulling, the connection
    /// is now closing, and the edge dies with it.
    Overflow,
}

/// Sender half of a connection's inbox.
#[derive(Clone)]
pub struct InboxSender {
    tx: tokio::sync::mpsc::Sender<InboxItem>,
}

/// Receiver half; the connection program's `each` drains this.
pub struct InboxReceiver {
    rx: tokio::sync::mpsc::Receiver<InboxItem>,
}

/// A bounded inbox pair for one connection.
pub fn inbox() -> (InboxSender, InboxReceiver) {
    let (tx, rx) = tokio::sync::mpsc::channel(INBOX_BOUND);
    (InboxSender { tx }, InboxReceiver { rx })
}

impl InboxSender {
    /// Non-blocking on purpose: a publisher's pump never waits on a
    /// connection. Full means the connection has chosen to fall behind.
    pub fn push(&self, item: InboxItem) -> Result<(), DeliveryRefused> {
        self.tx.try_send(item).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => DeliveryRefused::Overflow,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => DeliveryRefused::Gone,
        })
    }
}

impl InboxReceiver {
    /// The next item, or None once the wire closed and drained.
    pub async fn next(&mut self) -> Option<InboxItem> {
        self.rx.recv().await
    }
}

/// Node-local connections by id: who can still be delivered to HERE.
/// An id that is not present is Gone, and Gone prunes edges; after a
/// failover every edge pointing at this node's old connections prunes
/// on first delivery, which is the expected-stale story.
#[derive(Default)]
pub struct ConnectionRegistry {
    inner: Mutex<HashMap<String, InboxSender>>,
}

impl ConnectionRegistry {
    /// Registers a freshly upgraded connection's inbox.
    pub fn register(&self, id: &str, sender: InboxSender) {
        self.inner
            .lock()
            .expect("no panics hold the registry")
            .insert(id.to_owned(), sender);
    }

    /// Removes a closed connection; later deliveries report Gone.
    pub fn unregister(&self, id: &str) {
        self.inner
            .lock()
            .expect("no panics hold the registry")
            .remove(id);
    }

    /// Delivers one item to a connection, or says why not. Overflow
    /// also unregisters: the connection is closing, so subsequent
    /// deliveries see Gone instead of racing a dying inbox.
    pub fn deliver(&self, id: &str, item: InboxItem) -> Result<(), DeliveryRefused> {
        let mut inner = self.inner.lock().expect("no panics hold the registry");
        let Some(sender) = inner.get(id) else {
            return Err(DeliveryRefused::Gone);
        };
        match sender.push(item) {
            Ok(()) => Ok(()),
            Err(refused) => {
                inner.remove(id);
                Err(refused)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(topic: &str) -> InboxItem {
        InboxItem::Event {
            topic: topic.to_owned(),
            from_class: "Hub".to_owned(),
            from_name: "town".to_owned(),
            data: serde_json::json!({ "kind": "test" }),
        }
    }

    #[tokio::test]
    async fn deliveries_reach_a_registered_inbox_in_order() {
        let registry = ConnectionRegistry::default();
        let (tx, mut rx) = inbox();
        registry.register("conn#1", tx);

        registry.deliver("conn#1", event("first")).expect("lands");
        registry
            .deliver(
                "conn#1",
                InboxItem::Frame(serde_json::json!({ "event": "join" })),
            )
            .expect("frames merge into the same inbox");

        let first = rx.next().await.expect("item");
        assert!(matches!(first, InboxItem::Event { ref topic, .. } if topic == "first"));
        let second = rx.next().await.expect("item");
        assert!(matches!(second, InboxItem::Frame(_)));
    }

    #[tokio::test]
    async fn an_unknown_connection_is_gone_and_gone_means_prune() {
        let registry = ConnectionRegistry::default();
        assert_eq!(
            registry.deliver("conn#ghost", event("x")),
            Err(DeliveryRefused::Gone)
        );

        let (tx, rx) = inbox();
        registry.register("conn#2", tx);
        registry.unregister("conn#2");
        drop(rx);
        assert_eq!(
            registry.deliver("conn#2", event("x")),
            Err(DeliveryRefused::Gone)
        );
    }

    #[tokio::test]
    async fn overflow_closes_instead_of_buffering() {
        let registry = ConnectionRegistry::default();
        let (tx, _rx) = inbox();
        registry.register("conn#3", tx);

        let mut refused = None;
        for n in 0..=INBOX_BOUND {
            if let Err(reason) = registry.deliver("conn#3", event(&format!("e{n}"))) {
                refused = Some(reason);
                break;
            }
        }
        assert_eq!(refused, Some(DeliveryRefused::Overflow));
        // Overflow unregistered it: the next delivery is Gone, so a
        // pump prunes rather than retrying a dying connection.
        assert_eq!(
            registry.deliver("conn#3", event("late")),
            Err(DeliveryRefused::Gone)
        );
    }

    #[tokio::test]
    async fn a_closed_wire_still_drains_then_ends() {
        let (tx, mut rx) = inbox();
        tx.push(event("last")).expect("lands");
        tx.push(InboxItem::Closed).expect("lands");
        drop(tx);

        assert!(matches!(rx.next().await, Some(InboxItem::Event { .. })));
        assert_eq!(rx.next().await, Some(InboxItem::Closed));
        assert_eq!(rx.next().await, None);
    }
}
