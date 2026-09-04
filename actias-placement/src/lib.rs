//! The placement store as a library, so a service that needs a registry
//! beside its own tests (script-service's orphan fallback) can serve one
//! in-process over a test database.

pub mod migrate;
pub mod postgres;
pub mod registry;
pub mod scylla;
pub mod store;

pub mod proto_node_registry {
    tonic::include_proto!("node_registry");
}
