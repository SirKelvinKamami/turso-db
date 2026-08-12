mod analytics;
mod auth;
mod config;
mod db;
mod models;
mod plans;
mod ratelimit;
mod routes;
mod users;

use std::sync::Arc;
use axum::{Router, response::Redirect, routing::get};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::analytics::QueryTracker;
use crate::config::Config;
use crate::db::DatabaseManager;
use crate::users::UserStore;
use crate::ratelimit::RateLimiter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "turso_service=info,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Turso Service v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Data directory: {}", config.data_dir);

    let db_manager = DatabaseManager::new(&config.data_dir).await?;

    let user_store = UserStore::new(&config.data_dir)?;

    for (username, password) in &config.seed_users {
        match user_store.create_user(username, password) {
            Ok(u) => tracing::info!("Seeded user: {}", u.username),
            Err(e) => tracing::warn!("Seed user {}: {}", username, e),
        }
    }

    let user_store_arc = Arc::new(user_store);

    let rate_limiter = RateLimiter::new(config.max_queries_per_minute, 60);
    let query_tracker = QueryTracker::new();

    let app = Router::new()
        .route("/dashboard", get(|| async { Redirect::permanent("/dashboard.html") }))
        .nest("/v1", routes::api_routes(db_manager.clone(), (*user_store_arc).clone(), rate_limiter, query_tracker))
        .fallback_service(ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&config.bind_address).await?;
    tracing::info!("Server listening on {}", config.bind_address);
    tracing::info!("Web UI: http://{}/", config.bind_address);
    tracing::info!("Health check: http://{}/v1/health", config.bind_address);

    axum::serve(listener, app).await?;
    Ok(())
}
