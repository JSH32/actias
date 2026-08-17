use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub script_service_uri: String,
    pub kv_service_uri: String,
    /// Largest request body a script can be handed, in bytes.
    pub max_body_bytes: usize,
    /// Whole-request deadline, covering script lookup and execution.
    pub request_timeout_secs: u64,
    /// How long a cached identifier-to-script pointer may be served before
    /// re-resolving; the upper bound on publish propagation delay.
    pub pointer_ttl_secs: u64,
    /// Byte budget for the prepared revision cache.
    pub revision_cache_bytes: u64,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            script_service_uri: get_env("SCRIPT_SERVICE_URI"),
            kv_service_uri: get_env("KV_SERVICE_URI"),
            max_body_bytes: get_env_or("MAX_BODY_BYTES", 10 * 1024 * 1024),
            request_timeout_secs: get_env_or("REQUEST_TIMEOUT_SECS", 30),
            pointer_ttl_secs: get_env_or("POINTER_TTL_SECS", 5),
            revision_cache_bytes: get_env_or::<u64>("REVISION_CACHE_MB", 128) * 1024 * 1024,
        }
    }
}
