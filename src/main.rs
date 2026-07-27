mod auth;
mod config;
mod db;
mod models;
mod routes;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;
use crate::db::DatabaseManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration
    let config = Config::load()?;

    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "turso_service=info,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Turso Service v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Data directory: {}", config.data_dir);

    // Initialize database manager
    let db_manager = DatabaseManager::new(&config.data_dir, &config.encryption_key).await?;

    // Build router with static files
    let app = Router::new()
        .nest("/v1", routes::api_routes(db_manager.clone()))
        .fallback_service(ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // Start server
    let listener = tokio::net::TcpListener::bind(&config.bind_address).await?;
    tracing::info!("Server listening on {}", config.bind_address);
    tracing::info!("Web UI: http://{}/", config.bind_address);
    tracing::info!("Health check: http://{}/v1/health", config.bind_address);

    axum::serve(listener, app).await?;
    Ok(())
}
