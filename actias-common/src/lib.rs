use tracing::{subscriber::SetGlobalDefaultError, Level};
use tracing_subscriber::FmtSubscriber;

pub mod config;
pub use thiserror;
pub use tracing;

pub fn setup_tracing() -> Result<(), SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::DEBUG)
            .finish(),
    )
}
