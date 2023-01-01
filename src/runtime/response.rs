use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Lua HTTP response. Can be converted to and from lua.
#[derive(Serialize, Deserialize, Clone)]
pub struct Response {
    pub status_code: Option<u16>,
    pub body: Option<serde_json::Value>,
    pub headers: Option<HashMap<String, String>>,
}
