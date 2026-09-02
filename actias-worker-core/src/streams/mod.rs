//! Streams: publisher-approved edges between objects, with the platform
//! owning delivery.
//!
//! A follow writes one row in the publisher's own SQLite; a publish
//! appends to the publisher's event log in the calling transaction; the
//! delivery pump walks edge rows after commit, copying matching events
//! to each follower's `receives` handler with a per-edge cursor, retry
//! backoff, and bounded patience. Everything rides the object's file:
//! edges and events ship with snapshots and survive takeover like any
//! other rows.

use crate::storage::SqliteStorage;

mod pump;
pub use pump::*;
mod schema;
pub use schema::*;
mod wire;
pub use wire::*;
mod cursors;
pub use cursors::*;
mod events;
pub use events::*;
mod edges;
pub use edges::*;
#[cfg(test)]
mod tests;
