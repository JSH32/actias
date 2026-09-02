//! The class directory on the worker: the loops that turn settled rows
//! into content-addressed deltas and bases, the reader that answers
//! queries from a local overlay, and the repair passes that keep the
//! index a superset of the truth. The kernel (merge order, shape,
//! predicates, overlay files) lives in `actias_worker_core::directory`;
//! this module is the node's side of it.

pub mod backfill;
pub mod compact;
pub mod gauges;
pub mod query;
pub mod read;
pub mod rebuild;
pub mod route;
pub mod sweep;
pub mod sync;
pub mod visit;
