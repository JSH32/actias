use actias_common::config::{dotenv, get_env, get_env_or};

pub struct Config {
    pub port: u16,
    pub database_url: String,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Config {
            port: get_env_or("PORT", 3000),
            database_url: get_env("DATABASE_URL"),
        }
    }
}
