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
    /// Object storage holding bundle blobs; the worker pulls file bytes
    /// from here by hash instead of through script-service.
    pub s3_endpoint: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_bucket: String,
    /// Byte budget for the hash-keyed blob cache.
    pub blob_cache_bytes: u64,
    /// Address other platform services reach this node on, host:port;
    /// reported to the placement store at registration.
    pub node_address: String,
    /// Directory holding one SQLite file per durable object; a volume in
    /// any real deployment, since it is the objects' persistence.
    pub object_data_dir: String,
    /// Size cap per object database, bytes.
    pub object_db_max_bytes: u64,
    /// Domain scripts hang off as subdomains (`<ident>.<base>`); unset
    /// leaves only the path routing forms.
    pub base_domain: Option<String>,
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

        let port: u16 = get_env_or("PORT", 3000);

        Config {
            port,
            script_service_uri: get_env("SCRIPT_SERVICE_URI"),
            kv_service_uri: get_env("KV_SERVICE_URI"),
            redis_url: get_env("REDIS_URL"),
            secret_encryption_key: std::env::var("SECRET_ENCRYPTION_KEY").ok(),
            max_body_bytes: get_env_or("MAX_BODY_BYTES", 10 * 1024 * 1024),
            request_timeout_secs: get_env_or("REQUEST_TIMEOUT_SECS", 30),
            pointer_ttl_secs: get_env_or("POINTER_TTL_SECS", 5),
            revision_cache_bytes: get_env_or::<u64>("REVISION_CACHE_MB", 128) * 1024 * 1024,
            s3_endpoint: get_env("S3_ENDPOINT"),
            s3_access_key: get_env("S3_ACCESS_KEY"),
            s3_secret_key: get_env("S3_SECRET_KEY"),
            s3_bucket: get_env_or("S3_BUCKET", "actias-blobs".to_owned()),
            blob_cache_bytes: get_env_or::<u64>("BLOB_CACHE_MB", 256) * 1024 * 1024,
            // The container hostname resolves within a compose network,
            // which covers local; a deployment sets NODE_ADDRESS.
            node_address: get_env_or(
                "NODE_ADDRESS",
                format!(
                    "{}:{port}",
                    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned())
                ),
            ),
            object_data_dir: get_env_or("OBJECT_DATA_DIR", "./objects-data".to_owned()),
            object_db_max_bytes: get_env_or::<u64>("OBJECT_DB_MAX_MB", 64) * 1024 * 1024,
            base_domain: std::env::var("BASE_DOMAIN").ok().filter(|d| !d.is_empty()),
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
