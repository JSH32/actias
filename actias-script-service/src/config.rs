use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub redis_url: String,
    /// Object storage endpoint; only platform services reach it.
    pub s3_endpoint: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_bucket: String,
    /// Silence after which a worker node ages out of the registry.
    pub node_ttl_secs: u32,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            database_url: get_env("DATABASE_URL"),
            redis_url: get_env("REDIS_URL"),
            s3_endpoint: get_env("S3_ENDPOINT"),
            s3_access_key: get_env("S3_ACCESS_KEY"),
            s3_secret_key: get_env("S3_SECRET_KEY"),
            s3_bucket: get_env_or("S3_BUCKET", "actias-blobs".to_owned()),
            node_ttl_secs: get_env_or("NODE_TTL_SECS", 45),
        }
    }
}
