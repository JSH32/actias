use actix_web::{App, HttpServer};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[actix_web::main] // or #[tokio::main]
async fn main() -> std::io::Result<()> {
    // Setup tracing.
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let port = 8080;
    info!("Serving on 0.0.0.0:{}", port);

    HttpServer::new(|| App::new())
        .bind(("127.0.0.1", port))?
        .run()
        .await
}
