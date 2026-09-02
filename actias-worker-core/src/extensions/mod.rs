//! The capability surface a script can reach, one module per
//! capability. Each implements [`crate::runtime::extension::LuaExtension`].

pub mod crypto;
pub mod determinism;
pub mod http;
pub mod jwt;
pub mod kv;
pub mod log;
pub mod objects;
pub mod secrets;
pub mod sockets;
