//! The worker's operator interface: every knob it reads from the
//! environment, with the default each one falls back to.

use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    /// The region this node runs in, a region token: the fence every
    /// object's home is checked against, and what a call from another
    /// region is forwarded by.
    pub region: String,
    /// Where the WorkerData grpc service listens: the data plane peers
    /// and the api dispatch object calls and reads over.
    pub grpc_port: u16,
    pub script_service_uri: String,
    pub kv_service_uri: String,
    /// The region's placement service: claims, membership, alarms.
    pub placement_service_uri: String,
    /// Redis carrying script log lines to their subscribers.
    pub redis_url: String,
    /// Secret service address resolving `secret` declarations; unset
    /// disables secrets.
    pub secret_service_uri: Option<String>,
    /// Largest request body a script can be handed, in bytes.
    pub max_body_bytes: usize,
    /// Whole-request deadline, covering script lookup and execution.
    pub request_timeout_secs: u64,
    /// Work units one guest scope may spend: a request, an object call,
    /// a connection frame. What actually stops a runaway, since it
    /// counts work rather than time and so cannot be outrun by a busy
    /// host.
    pub guest_work_limit: u64,
    /// Wall backstop for the same scope, seconds. Only catches what the
    /// meter cannot see, code stuck outside the vm.
    pub guest_wall_secs: u64,
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
    /// The region's object bucket: objects and directories ship here,
    /// and a move copies between regions' buckets. Bundles stay in the
    /// control plane's `S3_BUCKET`, immutable and cached. Defaults to
    /// the same bucket, which is the single-region layout.
    pub object_bucket: String,
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
    /// A WAL this large rotates the shipping generation, bytes; the
    /// floor under the fraction.
    pub object_wal_rotate_bytes: u64,
    /// A WAL this fraction of the base's length rotates it too.
    pub object_wal_rotate_fraction: f64,
    /// Chunk puts and gets in flight at once per store operation.
    pub object_store_parallel: usize,
    /// So does this many shipped segments.
    pub object_max_segments: u32,
    /// Longest a written call's answer waits for its frames to reach the
    /// object store before the caller is told the outcome is unknown.
    pub object_ack_gate_ms: u64,
    /// Flights this node may have in the air at once; 0 is unbounded.
    /// Like every bound below, split fairly among the projects using it.
    pub object_ship_concurrency: usize,
    /// Requests this node runs at once; a project over its share is
    /// answered 429. 0 is unbounded.
    pub request_concurrency: usize,
    /// Blocking work (overlay builds, folds, candidate scans) at once.
    pub blocking_concurrency: usize,
    /// Open connections, both directions, at once.
    pub connection_limit: usize,
    /// Resident objects at once; a project over its share evicts its
    /// idlest object to make room.
    pub object_resident_limit: usize,
    /// Directory listings and visits at once.
    pub directory_query_concurrency: usize,
    /// The least share of any bound a project gets under contention, as
    /// a fraction of the bound.
    pub share_floor: f64,
    /// How many of those are held for writes a caller is waiting on.
    pub object_ship_reserved: usize,
    /// Idle seconds before a pinned object vm hibernates.
    pub object_idle_secs: u64,
    /// Warm vms kept per revision and flavor, built ahead of the request
    /// or object that takes them; 0 builds every vm inline.
    pub object_vm_pool: usize,
    /// Replica nodes an owner fans its WAL out to; 0 disables fan-out.
    pub object_replicas: usize,
    /// Replica acks that answer a written call; 0 is shadow mode, where
    /// the fan-out runs and the store's manifest stays the release.
    pub object_quorum: usize,
    /// How a replica makes an append durable before acking: "fsync" or
    /// "os".
    pub object_replica_sync: String,
    /// Longest an owner waits for one replica's ack, milliseconds.
    pub object_replica_ack_ms: u64,
    /// Idle seconds after which a replica copy the store covers leaves
    /// the disk.
    pub object_replica_idle_secs: u64,
    /// Idle seconds before a connection's vm hibernates; 0 never does.
    pub connection_hibernate_secs: u64,
    /// Deliveries attempted before a queue message dead-letters.
    pub queue_max_attempts: i64,
    /// First queue retry delay in milliseconds; doubles per attempt.
    pub queue_backoff_base_ms: i64,
    /// Seconds between cold-alarm sweeps of the object data dir.
    pub object_sweep_secs: u64,
    /// Milliseconds between directory delta flushes. The interval is
    /// the coalescing window: every settled row a node collects inside
    /// one becomes a single upload per class, so raising it trades
    /// index freshness for fewer, larger deltas.
    pub directory_flush_ms: u64,
    /// Milliseconds a `directory` function may run before its budget
    /// is spent. Contained like any other failure: the write commits,
    /// the last good row stays, the failure is marked.
    pub directory_eval_budget_ms: u64,
    /// Seconds between directory compaction passes. Folding is the only
    /// serialized step in the directory and it is off the write path,
    /// so this trades query freshness against store traffic.
    pub directory_compact_secs: u64,
    /// Seconds between directory reconciliation passes: rows recovered
    /// from object manifests, and rows retired for objects that no
    /// longer exist. One GET per object, so this is rare on purpose.
    /// It bounds how long a missing or ghost row can persist; it is not
    /// what keeps the index fresh.
    pub directory_rebuild_secs: u64,
    /// Seconds between crash-sweep polls. Frequent and cheap: the poll
    /// is one indexed query that answers nothing on a healthy cluster,
    /// and a dead node's rows should not wait a reconciliation interval.
    pub directory_sweep_secs: u64,
    /// Seconds an unused directory overlay stays on disk. Pure cache
    /// rebuilt from immutable files, so evicting costs a rebuild and
    /// never correctness; holding every class a node was ever asked
    /// about is what costs disk forever.
    pub directory_overlay_ttl_secs: u64,
    /// Byte budget for cached directory bases and deltas. They are
    /// content-addressed, so an entry can never be stale and the cache
    /// is what keeps a hot class from re-downloading its whole base on
    /// every compaction and every overlay rebuild.
    pub directory_cache_bytes: u64,
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
        let region: String = get_env_or("REGION", "local".to_owned());
        assert!(
            actias_common::naming::is_region_token(&region),
            "REGION '{region}' is not a region token: 1 to 16 of a-z, 0-9 and '-', not starting with '-'"
        );

        Config {
            port,
            region,
            grpc_port,
            script_service_uri: get_env("SCRIPT_SERVICE_URI"),
            kv_service_uri: get_env("KV_SERVICE_URI"),
            placement_service_uri: get_env("PLACEMENT_SERVICE_URI"),
            redis_url: get_env("REDIS_URL"),
            secret_service_uri: std::env::var("SECRET_SERVICE_URI").ok(),
            max_body_bytes: get_env_or("MAX_BODY_BYTES", 10 * 1024 * 1024),
            request_timeout_secs: get_env_or("REQUEST_TIMEOUT_SECS", 30),
            guest_work_limit: get_env_or(
                "GUEST_WORK_LIMIT",
                actias_worker_core::budget::DEFAULT_WORK_LIMIT,
            ),
            guest_wall_secs: get_env_or("GUEST_WALL_SECS", 10),
            pointer_ttl_secs: get_env_or("POINTER_TTL_SECS", 5),
            revision_cache_bytes: get_env_or::<u64>("REVISION_CACHE_MB", 128) * 1024 * 1024,
            s3_endpoint: get_env("S3_ENDPOINT"),
            s3_access_key: get_env("S3_ACCESS_KEY"),
            s3_secret_key: get_env("S3_SECRET_KEY"),
            s3_bucket: get_env_or("S3_BUCKET", "actias-blobs".to_owned()),
            object_bucket: get_env_or(
                "OBJECT_BUCKET",
                get_env_or("S3_BUCKET", "actias-blobs".to_owned()),
            ),
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
            object_db_max_bytes: get_env_or::<u64>("OBJECT_DB_MAX_MB", 1024) * 1024 * 1024,
            object_wal_rotate_bytes: get_env_or::<u64>("OBJECT_WAL_ROTATE_KB", 4096) * 1024,
            object_wal_rotate_fraction: get_env_or("OBJECT_WAL_ROTATE_FRACTION", 0.125),
            object_store_parallel: get_env_or("OBJECT_STORE_PARALLEL", 8),
            object_max_segments: get_env_or("OBJECT_MAX_SEGMENTS", 64),
            object_ack_gate_ms: get_env_or("OBJECT_ACK_GATE_MS", 10_000),
            object_ship_concurrency: get_env_or("OBJECT_SHIP_CONCURRENCY", 32),
            request_concurrency: get_env_or("REQUEST_CONCURRENCY", 1024),
            blocking_concurrency: get_env_or("BLOCKING_CONCURRENCY", 64),
            connection_limit: get_env_or("CONNECTION_LIMIT", 4096),
            object_resident_limit: get_env_or("OBJECT_RESIDENT_LIMIT", 10_000),
            directory_query_concurrency: get_env_or("DIRECTORY_QUERY_CONCURRENCY", 64),
            share_floor: get_env_or("SHARE_FLOOR", 0.05),
            object_ship_reserved: get_env_or("OBJECT_SHIP_RESERVED", 8),
            object_idle_secs: get_env_or("OBJECT_IDLE_SECS", 300),
            object_vm_pool: get_env_or("OBJECT_VM_POOL", 4),
            object_replicas: get_env_or("OBJECT_REPLICAS", 3),
            object_quorum: get_env_or("OBJECT_QUORUM", 2),
            object_replica_sync: get_env_or("OBJECT_REPLICA_SYNC", "fsync".to_owned()),
            object_replica_ack_ms: get_env_or("OBJECT_REPLICA_ACK_MS", 2000),
            object_replica_idle_secs: get_env_or("OBJECT_REPLICA_IDLE_SECS", 1800),
            connection_hibernate_secs: get_env_or("CONNECTION_HIBERNATE_SECS", 300),
            queue_max_attempts: get_env_or("QUEUE_MAX_ATTEMPTS", 5),
            queue_backoff_base_ms: get_env_or("QUEUE_BACKOFF_BASE_MS", 2000),
            object_sweep_secs: get_env_or("OBJECT_SWEEP_SECS", 30),
            directory_flush_ms: get_env_or("DIRECTORY_FLUSH_MS", 200),
            directory_compact_secs: get_env_or("DIRECTORY_COMPACT_SECS", 10),
            directory_rebuild_secs: get_env_or("DIRECTORY_REBUILD_SECS", 900),
            directory_sweep_secs: get_env_or("DIRECTORY_SWEEP_SECS", 15),
            directory_overlay_ttl_secs: get_env_or("DIRECTORY_OVERLAY_TTL_SECS", 1800),
            directory_cache_bytes: get_env_or::<u64>("DIRECTORY_CACHE_MB", 256) * 1024 * 1024,
            directory_eval_budget_ms: get_env_or(
                "DIRECTORY_EVAL_BUDGET_MS",
                actias_worker_core::directory::DEFAULT_EVAL_BUDGET_MS,
            ),
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
