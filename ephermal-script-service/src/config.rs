use ephermal_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub mongo_uri: String,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            mongo_uri: get_env("MONGO_URI"),
        }
    }
}
