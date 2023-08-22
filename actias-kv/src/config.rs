use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub scylla_nodes: Vec<String>,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            scylla_nodes: get_env::<String>("SCYLLA_NODES")
                .split(",")
                .collect::<Vec<&str>>()
                .iter()
                .map(|&s| s.into())
                .collect(),
        }
    }
}
