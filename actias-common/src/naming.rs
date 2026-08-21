//! Reserved names the platform keeps for itself across services.

/// Kv namespaces starting with this prefix belong to the platform; scripts
/// cannot declare them and the api does not serve them. (The `__secrets`
/// namespace once lived here; secrets moved to their own service, and any
/// remaining rows are orphaned, unreachable data.)
pub const RESERVED_NAMESPACE_PREFIX: &str = "__";
