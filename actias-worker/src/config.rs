use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub script_service_uri: String,
    pub kv_service_uri: String,
    /// Redis carrying script log lines to their subscribers.
    pub redis_url: String,
    /// Base64 AES-256 key decrypting stored secrets; unset disables secrets.
    pub secret_encryption_key: Option<String>,
    /// Largest request body a script can be handed, in bytes.
    pub max_body_bytes: usize,
    /// Whole-request deadline, covering script lookup and execution.
    pub request_timeout_secs: u64,
    /// How long a cached identifier-to-script pointer may be served before
    /// re-resolving; the upper bound on publish propagation delay.
    pub pointer_ttl_secs: u64,
    /// Byte budget for the prepared revision cache.
    pub revision_cache_bytes: u64,
    /// Hostnames scripts may never reach, beyond the service uris the worker
    /// already knows; comma separated.
    pub egress_denied_hosts: Vec<String>,
    /// Permits outbound requests to private and local addresses; for local
    /// development only.
    pub egress_allow_private: bool,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            script_service_uri: get_env("SCRIPT_SERVICE_URI"),
            kv_service_uri: get_env("KV_SERVICE_URI"),
            redis_url: get_env("REDIS_URL"),
            secret_encryption_key: std::env::var("SECRET_ENCRYPTION_KEY").ok(),
            max_body_bytes: get_env_or("MAX_BODY_BYTES", 10 * 1024 * 1024),
            request_timeout_secs: get_env_or("REQUEST_TIMEOUT_SECS", 30),
            pointer_ttl_secs: get_env_or("POINTER_TTL_SECS", 5),
            revision_cache_bytes: get_env_or::<u64>("REVISION_CACHE_MB", 128) * 1024 * 1024,
            egress_denied_hosts: get_env_or("EGRESS_DENIED_HOSTS", String::new())
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(str::to_owned)
                .collect(),
            egress_allow_private: get_env_or("EGRESS_ALLOW_PRIVATE", false),
        }
    }
}
