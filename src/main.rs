mod analytics;
mod auth;
mod config;
mod db;
mod libsql;
mod models;
mod plans;
mod ratelimit;
mod routes;
mod supabase;
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
use crate::supabase::Supabase;
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

    let supabase = Supabase::from_env();
    if supabase.is_some() {
        tracing::info!("Supabase persistence enabled");
    } else {
        tracing::warn!("SUPABASE_URL/SUPABASE_SERVICE_KEY not set - using ephemeral storage");
    }

    let db_manager = DatabaseManager::new(&config.data_dir, supabase.clone()).await?;

    let user_store = UserStore::new(supabase.clone());

    for (username, password) in &config.seed_users {
        match user_store.create_user(username, password).await {
            Ok(u) => tracing::info!("Seeded user: {}", u.username),
            Err(e) => tracing::warn!("Seed user {}: {}", username, e),
        }
    }

    let user_store_arc = Arc::new(user_store);

    let rate_limiter = RateLimiter::new(config.max_queries_per_minute, 60);
    let query_tracker = QueryTracker::new(supabase.clone());
    if supabase.is_some() {
        query_tracker.load_from_supabase().await;
        let tracker = query_tracker.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(analytics::FLUSH_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let flushed = tracker.flush().await;
                if flushed > 0 {
                    tracing::info!("Flushed analytics for {} user(s)", flushed);
                }
            }
        });
    }

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
