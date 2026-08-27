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

/// The interceptor shape platform grpc clients carry: a plain `fn`, so
/// the composed client type stays nameable in signatures. The behavior
/// (trace-context injection) lives in actias-common; this crate only
/// names the shape.
pub type GrpcInterceptor = fn(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status>;

/// The channel every platform grpc client runs over: a transport
/// channel behind the nameable interceptor.
pub type Grpc =
    tonic::service::interceptor::InterceptedService<tonic::transport::Channel, GrpcInterceptor>;

/// A channel behind a do-nothing interceptor, for tests and callers
/// with no tracing wired.
pub fn plain_grpc(channel: tonic::transport::Channel) -> Grpc {
    // The Err size is tonic's Interceptor contract.
    #[allow(clippy::result_large_err)]
    fn identity(request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        Ok(request)
    }
    tonic::service::interceptor::InterceptedService::new(channel, identity as GrpcInterceptor)
}

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
