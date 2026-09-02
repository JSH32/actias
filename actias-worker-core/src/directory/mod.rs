//! The directory: one queryable row per object, derived from the
//! class's `directory` function. This module is the kernel only:
//! merge order, declared shape, predicate translation, and the
//! reserved table in the object's own file. Evaluation wiring,
//! shipping and the read path live with their owners (dispatch,
//! shipper, worker) and consume these types.

pub mod compact;
pub mod delta;
pub mod evaluate;
pub mod manifest;
pub mod overlay;
pub mod predicate;
pub mod repair;
pub mod row;
pub mod scratch;
pub mod shape;
pub mod verify;
pub mod version;

/// The identity checksum lives with the other spellings the placement
/// store and this kernel must agree on, so the fold has one definition
/// across the two services that compute it.
pub use actias_common::directory_identity as identity;

/// Largest encoded row, bytes: names plus values across every field.
/// The row rides the object's shipping manifest on every settled
/// flight, so its bound is stated in the unit that path pays. There
/// is no field-count cap; this is the honest limit behind it. An
/// oversized row is refused by [`row::record`] and contained by the
/// evaluation layer exactly like a throw.
pub const DEFAULT_ROW_MAX_BYTES: usize = 4096;

/// Compute budget for one `directory` evaluation, milliseconds.
/// Operator knob `DIRECTORY_EVAL_BUDGET_MS`. Exceeding it counts as a
/// throw and is contained the same way: the business write commits,
/// the row keeps its last good value, the failure is marked.
pub const DEFAULT_EVAL_BUDGET_MS: u64 = 5;
