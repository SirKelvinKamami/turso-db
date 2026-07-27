use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;

use crate::auth::{create_token, verify_token, extract_token_from_header, generate_api_key, TokenResponse};
use crate::config::Config;
use crate::db::DatabaseManager;
use crate::models::*;

pub fn api_routes(db_manager: DatabaseManager) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/auth/login", post(login))
        .route("/auth/google-token", post(google_token))
        .route("/databases", get(list_databases).post(create_database))
        .route("/databases/{id}", get(get_database).delete(delete_database))
        .route("/databases/{id}/execute", post(execute_query))
        .route("/databases/{id}/query", post(run_query))
        .with_state(db_manager)
}

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": Utc::now().to_rfc3339()
    }))
}

async fn login(Json(payload): Json<LoginRequest>) -> Result<Json<TokenResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = Config::load().expect("Failed to load config");
    
    let admin_user = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let admin_pass = std::env::var("ADMIN_PASSWORD").expect("ADMIN_PASSWORD must be set in .env");
    
    if payload.username == admin_user && payload.password == admin_pass {
        let token = create_token(&payload.username, &config.jwt_secret, config.jwt_expiry_hours)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "TOKEN_ERROR".into() })))?;
        Ok(Json(TokenResponse { token }))
    } else {
        Err((StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Invalid credentials".into(), code: "AUTH_FAILED".into() })))
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct GoogleTokenRequest {
    pub email: String,
    pub name: String,
}

async fn google_token(
    Json(payload): Json<GoogleTokenRequest>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = Config::load().expect("Failed to load config");
    
    let jwt_token = create_token(&payload.email, &config.jwt_secret, config.jwt_expiry_hours)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "JWT_ERROR".into() })))?;
    
    Ok(Json(UserResponse {
        id: generate_api_key(),
        email: payload.email,
        name: payload.name,
        token: jwt_token,
    }))
}

async fn list_databases(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    Ok(Json(db_manager.list_databases()))
}

async fn create_database(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
    Json(payload): Json<CreateDatabaseRequest>,
) -> Result<Json<DatabaseResponse>, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    
    let id = db_manager.create_database(&payload.name).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "CREATE_ERROR".into() })))?;
    
    Ok(Json(DatabaseResponse {
        id,
        name: payload.name,
        created_at: Utc::now().to_rfc3339(),
    }))
}

async fn get_database(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DatabaseResponse>, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    
    db_manager.get_database(&id).await
        .map_err(|_| (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Database not found".into(), code: "NOT_FOUND".into() })))?;
    
    Ok(Json(DatabaseResponse {
        id: id.clone(),
        name: format!("Database {}", id),
        created_at: Utc::now().to_rfc3339(),
    }))
}

async fn delete_database(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    
    db_manager.delete_database(&id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "DELETE_ERROR".into() })))?;
    
    Ok(StatusCode::NO_CONTENT)
}

async fn execute_query(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    
    let result = db_manager.execute(&id, &payload.sql).await
        .map_err(|e| {
            tracing::error!("Execute error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "EXECUTE_ERROR".into() }))
        })?;
    
    Ok(Json(ExecuteResponse {
        success: true,
        message: result,
    }))
}

async fn run_query(
    State(db_manager): State<DatabaseManager>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<ExecuteRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    authenticate(&headers)?;
    
    let rows = db_manager.query(&id, &payload.sql).await
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "QUERY_ERROR".into() }))
        })?;
    
    let columns = if !rows.is_empty() {
        (0..rows[0].len()).map(|i| format!("column_{}", i)).collect()
    } else {
        vec![]
    };
    
    Ok(Json(QueryResponse { columns, rows }))
}

fn authenticate(headers: &HeaderMap) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let config = Config::load().expect("Failed to load config");
    
    let auth_header = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Missing authorization header".into(), code: "UNAUTHORIZED".into() })))?;
    
    let token = extract_token_from_header(auth_header)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Invalid authorization format".into(), code: "UNAUTHORIZED".into() })))?;
    
    verify_token(&token, &config.jwt_secret)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Invalid or expired token".into(), code: "UNAUTHORIZED".into() })))?;
    
    Ok(())
}
