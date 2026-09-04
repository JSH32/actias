//! The script runtime, embeddable anywhere: vm construction, the extension
//! registry, prepared revisions with their bytecode, and the egress policy.
//!
//! `actias-worker` serves this over http in production; local tooling (the
//! CLI's test runner) embeds the same crate, so scripts behave identically
//! wherever they run.

pub mod budget;
pub mod connections;
pub mod directory;
pub mod drill;
pub mod egress;
pub mod extensions;
pub mod identity;
pub mod objects;
pub mod platform;
pub mod runtime;
pub mod shares;
pub mod storage;
pub mod streams;
pub mod wal;

/// The channel every platform grpc client runs over: a transport
/// channel that spans and propagates each call (a no-op until otel is
/// configured, see actias_common::otel).
pub type Grpc = actias_common::otel::TracedChannel;

/// Wraps a transport channel for the platform clients; the name exists
/// so call sites (tests included) read the same everywhere.
pub fn plain_grpc(channel: tonic::transport::Channel) -> Grpc {
    actias_common::otel::traced_channel(channel)
}

pub mod proto {
    /// The buffer type of the replication payloads.
    pub use prost::bytes::Bytes;

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
