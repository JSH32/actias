pub mod classes;
pub mod config;
pub mod logging;
pub mod naming;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "otel")]
pub mod otel;
pub use thiserror;
pub use tracing;

/// The one subscriber every service installs. RUST_LOG overrides the
/// default filter, which keeps platform crates at debug and dependency
/// noise (h2, hyper, the aws sdk) at info. With the `otel` feature and
/// OTEL_EXPORTER_OTLP_ENDPOINT set, spans also export over OTLP.
pub fn setup_tracing() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,actias_worker=debug,actias_worker_core=debug,actias_script_service=debug,\
             actias_kv=debug,actias_secret_service=debug,actias_api=debug,actias_common=debug",
        )
    });

    #[cfg(feature = "otel")]
    let otel_layer = otel::layer()?;
    #[cfg(not(feature = "otel"))]
    let otel_layer = None::<tracing_subscriber::layer::Identity>;

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .try_init()?;
    Ok(())
}
