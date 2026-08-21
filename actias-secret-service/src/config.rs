use actias_common::config::{dotenv, get_env, get_env_or};
use base64::Engine;
use zeroize::Zeroizing;

use crate::envelope::KEY_LEN;

pub struct Config {
    pub port: u16,
    pub database_url: String,
    /// The active master key and its label; every new write wraps under it.
    pub master_key_id: String,
    pub master_key: Zeroizing<[u8; KEY_LEN]>,
    /// A previous master kept readable during rotation; rows it wrapped
    /// still open, new writes never use it.
    pub previous_master: Option<(String, Zeroizing<[u8; KEY_LEN]>)>,
}

/// Decodes one base64 master key, panicking at startup (never in a request
/// path) when it is not exactly [`KEY_LEN`] bytes.
fn decode_key(var: &str, encoded: &str) -> Zeroizing<[u8; KEY_LEN]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_else(|_| panic!("{var} is not valid base64"));
    let key: [u8; KEY_LEN] = bytes
        .try_into()
        .unwrap_or_else(|_| panic!("{var} must decode to exactly {KEY_LEN} bytes"));
    Zeroizing::new(key)
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        let previous_master = std::env::var("SECRET_MASTER_KEY_PREVIOUS")
            .ok()
            .map(|encoded| {
                let id: String = get_env("SECRET_MASTER_KEY_PREVIOUS_ID");
                (id, decode_key("SECRET_MASTER_KEY_PREVIOUS", &encoded))
            });

        Config {
            port: get_env_or("PORT", 3000),
            database_url: get_env("DATABASE_URL"),
            master_key_id: get_env_or("SECRET_MASTER_KEY_ID", "kek-1".to_owned()),
            master_key: decode_key("SECRET_MASTER_KEY", &get_env::<String>("SECRET_MASTER_KEY")),
            previous_master,
        }
    }
}
