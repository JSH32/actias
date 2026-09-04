//! The placement service's operator interface: every knob it reads from
//! the environment, with the default each one falls back to, and which
//! store backs it.

use actias_common::config::{dotenv, get_env_or};

/// Which store backs this service, from the environment: postgres when
/// DATABASE_URL is set (the small-stack posture), scylla when
/// SCYLLA_NODES is; PLACEMENT_BACKEND breaks a tie when a deployment
/// sets both.
pub enum Backend {
    Postgres(String),
    Scylla {
        nodes: Vec<String>,
        /// The datacenter the keyspace replicates in, and how many times.
        dc: String,
        replication_factor: u32,
    },
}

pub struct Config {
    pub port: u16,
    pub backend: Backend,
    /// The region this store serves; the partition every regional table
    /// keys by. A region token: one to sixteen of `a-z`, `0-9` and `-`,
    /// not starting with `-`.
    pub region: String,
    /// Silence after which a node has aged out and its leases are free.
    pub node_ttl_secs: u32,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        let database_url = std::env::var("DATABASE_URL").ok();
        let scylla_nodes = std::env::var("SCYLLA_NODES").ok();
        let chosen = std::env::var("PLACEMENT_BACKEND").ok();
        let scylla = |nodes: &str| Backend::Scylla {
            nodes: nodes.split(',').map(|s| s.trim().to_owned()).collect(),
            dc: get_env_or("SCYLLA_DC", "datacenter1".to_owned()),
            replication_factor: get_env_or("SCYLLA_REPLICATION_FACTOR", 1),
        };
        let backend = match (chosen.as_deref(), database_url, scylla_nodes) {
            (Some("postgres"), Some(url), _) => Backend::Postgres(url),
            (Some("scylla"), _, Some(nodes)) => scylla(&nodes),
            (Some(other), _, _) => panic!(
                "PLACEMENT_BACKEND '{other}' needs its url: postgres wants DATABASE_URL, \
                 scylla wants SCYLLA_NODES"
            ),
            (None, Some(url), None) => Backend::Postgres(url),
            (None, None, Some(nodes)) => scylla(&nodes),
            (None, Some(_), Some(_)) => {
                panic!(
                    "Both DATABASE_URL and SCYLLA_NODES are set; pick one with PLACEMENT_BACKEND"
                )
            }
            (None, None, None) => {
                panic!(
                    "The placement service needs DATABASE_URL (postgres) or SCYLLA_NODES (scylla)"
                )
            }
        };

        Config {
            port: get_env_or("PORT", 3000),
            backend,
            region: {
                let region: String = get_env_or("REGION", "local".to_owned());
                assert!(
                    actias_common::naming::is_region_token(&region),
                    "REGION '{region}' is not a region token: 1 to 16 of a-z, 0-9 and '-', not starting with '-'"
                );
                region
            },
            node_ttl_secs: get_env_or("NODE_TTL_SECS", 45),
        }
    }
}
