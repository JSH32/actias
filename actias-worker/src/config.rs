use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    /// Where the WorkerData grpc service listens: the data plane peers
    /// and the api dispatch object calls and reads over.
    pub grpc_port: u16,
    pub script_service_uri: String,
    pub kv_service_uri: String,
    /// Redis carrying script log lines to their subscribers.
    pub redis_url: String,
    /// Secret service address resolving `secret` declarations; unset
    /// disables secrets.
    pub secret_service_uri: Option<String>,
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
    /// Address other platform services reach this node's WorkerData grpc
    /// service on, host:port; reported to the placement store at
    /// registration.
    pub node_address: String,
    /// Directory holding one SQLite file per durable object; a volume in
    /// any real deployment, since it is the objects' persistence.
    pub object_data_dir: String,
    /// Size cap per object database, bytes.
    pub object_db_max_bytes: u64,
    /// Databases at or past this size ship WAL segments instead of the
    /// whole file, bytes.
    pub object_ship_whole_max_bytes: u64,
    /// A WAL this large rotates the shipping generation, bytes.
    pub object_wal_rotate_bytes: u64,
    /// So does this many shipped segments.
    pub object_max_segments: u32,
    /// Longest a written call's answer waits for its frames to reach the
    /// object store before the caller is told the outcome is unknown.
    pub object_ack_gate_ms: u64,
    /// Idle seconds before a pinned object vm hibernates.
    pub object_idle_secs: u64,
    /// Idle seconds before a connection's vm hibernates; 0 never does.
    pub connection_hibernate_secs: u64,
    /// Deliveries attempted before a queue message dead-letters.
    pub queue_max_attempts: i64,
    /// First queue retry delay in milliseconds; doubles per attempt.
    pub queue_backoff_base_ms: i64,
    /// Seconds between cold-alarm sweeps of the object data dir.
    pub object_sweep_secs: u64,
    /// Shared secret authenticating node-to-node object forwards.
    pub internal_token: String,
    /// Seconds a snapshot replica serves reads before refreshing.
    pub replica_ttl_secs: u64,
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
        let grpc_port: u16 = get_env_or("WORKER_GRPC_PORT", 3100);

        Config {
            port,
            grpc_port,
            script_service_uri: get_env("SCRIPT_SERVICE_URI"),
            kv_service_uri: get_env("KV_SERVICE_URI"),
            redis_url: get_env("REDIS_URL"),
            secret_service_uri: std::env::var("SECRET_SERVICE_URI").ok(),
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
                    "{}:{grpc_port}",
                    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned())
                ),
            ),
            object_data_dir: get_env_or("OBJECT_DATA_DIR", "./objects-data".to_owned()),
            object_db_max_bytes: get_env_or::<u64>("OBJECT_DB_MAX_MB", 64) * 1024 * 1024,
            object_ship_whole_max_bytes: get_env_or::<u64>("OBJECT_SHIP_WHOLE_MAX_KB", 256) * 1024,
            object_wal_rotate_bytes: get_env_or::<u64>("OBJECT_WAL_ROTATE_KB", 4096) * 1024,
            object_max_segments: get_env_or("OBJECT_MAX_SEGMENTS", 64),
            object_ack_gate_ms: get_env_or("OBJECT_ACK_GATE_MS", 10_000),
            object_idle_secs: get_env_or("OBJECT_IDLE_SECS", 300),
            connection_hibernate_secs: get_env_or("CONNECTION_HIBERNATE_SECS", 300),
            queue_max_attempts: get_env_or("QUEUE_MAX_ATTEMPTS", 5),
            queue_backoff_base_ms: get_env_or("QUEUE_BACKOFF_BASE_MS", 2000),
            object_sweep_secs: get_env_or("OBJECT_SWEEP_SECS", 30),
            // Development default; a deployment must set its own.
            internal_token: get_env_or("INTERNAL_TOKEN", "dev-internal-token".to_owned()),
            replica_ttl_secs: get_env_or("OBJECT_REPLICA_TTL_SECS", 30),
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
