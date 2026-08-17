//! Reserved names the platform keeps for itself across services.

/// Kv namespaces starting with this prefix belong to the platform; scripts
/// cannot declare them and the api does not serve them.
pub const RESERVED_NAMESPACE_PREFIX: &str = "__";

/// The reserved kv namespace holding a project's encrypted secrets.
///
/// Values are `base64(nonce || ciphertext || tag)` under AES-256-GCM with the
/// deployment's `SECRET_ENCRYPTION_KEY`; the api writes them, the worker
/// reads and decrypts them. The same format is implemented in
/// `actias-api/src/secrets`, and the worker carries a cross-language test
/// vector pinning it.
pub const SECRETS_NAMESPACE: &str = "__secrets";
