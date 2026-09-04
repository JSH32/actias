//! The script service's operator interface: every knob it reads from
//! the environment.

use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub database_url: String,
    /// A read replica for what workers ask at runtime (script pointers,
    /// revisions, aliases, policy, regions); the primary when unset.
    /// The console's own listings stay on the primary, so a publish is
    /// visible to the publisher at once.
    pub read_database_url: Option<String>,
    pub redis_url: String,
    /// Object storage endpoint; only platform services reach it.
    pub s3_endpoint: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_bucket: String,
    /// Silence after which a worker node ages out of the registry.
    /// The placement service the instance directory is read from.
    pub placement_service_uri: String,
    /// The control plane's own region, a region token: the home of a
    /// project that was not given one.
    pub region: String,
    /// How long a move waits after marking a project moving before it
    /// copies: one worker pointer ttl plus one sweep, so every residency
    /// of the scope has ended (FLEET.md 6.3 step 2).
    pub move_drain_secs: u64,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            database_url: get_env("DATABASE_URL"),
            read_database_url: std::env::var("READ_DATABASE_URL")
                .ok()
                .filter(|url| !url.trim().is_empty()),
            redis_url: get_env("REDIS_URL"),
            s3_endpoint: get_env("S3_ENDPOINT"),
            s3_access_key: get_env("S3_ACCESS_KEY"),
            s3_secret_key: get_env("S3_SECRET_KEY"),
            s3_bucket: get_env_or("S3_BUCKET", "actias-blobs".to_owned()),
            placement_service_uri: get_env("PLACEMENT_SERVICE_URI"),
            move_drain_secs: get_env_or("MOVE_DRAIN_SECS", 40),
            region: {
                let region: String = get_env_or("REGION", "local".to_owned());
                assert!(
                    actias_common::naming::is_region_token(&region),
                    "REGION '{region}' is not a region token: 1 to 16 of a-z, 0-9 and '-', not starting with '-'"
                );
                region
            },
        }
    }
}
