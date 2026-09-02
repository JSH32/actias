//! The kv service's operator interface, including which store backs it.

use actias_common::config::{dotenv, get_env_or};

/// Which store backs this service, from the environment: postgres when
/// DATABASE_URL is set (the default posture), scylla when SCYLLA_NODES
/// is; KV_BACKEND breaks a tie when a deployment sets both.
pub enum Backend {
    Postgres(String),
    Scylla(Vec<String>),
}

pub struct Config {
    pub port: u16,
    pub backend: Backend,
    /// Seconds between expired-row sweeps; postgres only.
    pub sweep_secs: u64,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        let database_url = std::env::var("DATABASE_URL").ok();
        let scylla_nodes = std::env::var("SCYLLA_NODES").ok();
        let chosen = std::env::var("KV_BACKEND").ok();

        let backend = match (chosen.as_deref(), database_url, scylla_nodes) {
            (Some("postgres"), Some(url), _) => Backend::Postgres(url),
            (Some("scylla"), _, Some(nodes)) => Backend::Scylla(split(&nodes)),
            (Some(other), _, _) => panic!(
                "KV_BACKEND '{other}' needs its url: postgres wants DATABASE_URL, \
                 scylla wants SCYLLA_NODES"
            ),
            (None, Some(url), None) => Backend::Postgres(url),
            (None, None, Some(nodes)) => Backend::Scylla(split(&nodes)),
            (None, Some(_), Some(_)) => {
                panic!("Both DATABASE_URL and SCYLLA_NODES are set; pick one with KV_BACKEND")
            }
            (None, None, None) => {
                panic!("The kv service needs DATABASE_URL (postgres) or SCYLLA_NODES (scylla)")
            }
        };

        Config {
            port: get_env_or("PORT", 3000),
            backend,
            sweep_secs: get_env_or("KV_SWEEP_SECS", 60),
        }
    }
}

fn split(nodes: &str) -> Vec<String> {
    nodes.split(',').map(|s| s.trim().to_owned()).collect()
}
