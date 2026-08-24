//! The script runtime, embeddable anywhere: vm construction, the extension
//! registry, prepared revisions with their bytecode, and the egress policy.
//!
//! `actias-worker` serves this over http in production; local tooling (the
//! CLI's test runner) embeds the same crate, so scripts behave identically
//! wherever they run.

pub mod connections;
pub mod egress;
pub mod extensions;
pub mod identity;
pub mod objects;
pub mod platform;
pub mod runtime;
pub mod storage;
pub mod streams;

pub mod proto {
    pub mod bundle {
        tonic::include_proto!("bundle");
    }

    pub mod script_service {
        tonic::include_proto!("script_service");
    }

    pub mod kv_service {
        tonic::include_proto!("kv_service");
    }

    pub mod node_registry {
        tonic::include_proto!("node_registry");
    }

    pub mod secret_service {
        tonic::include_proto!("secret_service");
    }

    pub mod worker_data {
        tonic::include_proto!("worker_data");
    }
}
