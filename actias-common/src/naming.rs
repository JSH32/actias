//! Reserved names the platform keeps for itself.

/// Kv namespaces starting with this prefix belong to the platform, so a
/// script cannot declare one. Nothing occupies the prefix today; holding
/// it is what lets the platform take a namespace later without breaking
/// a script that got there first, and it matches the same reservation on
/// object classes ([`crate::classes`]), the `__actias_` tables inside an
/// object's storage, and the worker's `_` url namespace.
pub const RESERVED_NAMESPACE_PREFIX: &str = "__";
