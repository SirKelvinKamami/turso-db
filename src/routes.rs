use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put, delete},
    Json, Router,
};
use chrono::Utc;

use crate::analytics::{QueryTracker, AnalyticsResponse, VolumePoint};
use crate::auth::{create_token, verify_token, extract_token_from_header, generate_api_key, is_admin, Claims};
use crate::config::Config;
use crate::db::DatabaseManager;
use crate::models::*;
use crate::plans::Plan;
use crate::ratelimit::RateLimiter;
use crate::users::UserStore;

#[derive(Clone)]
pub struct AppState {
    pub db_manager: DatabaseManager,
    pub user_store: UserStore,
    pub rate_limiter: RateLimiter,
    pub query_tracker: QueryTracker,
}

pub fn api_routes(db_manager: DatabaseManager, user_store: UserStore, rate_limiter: RateLimiter, query_tracker: QueryTracker) -> Router {
    let state = AppState { db_manager, user_store, rate_limiter, query_tracker };

    Router::new()
        .route("/health", get(health_check))
        .route("/auth/login", post(login))
        .route("/auth/google-token", post(google_token))
        .route("/auth/signup", post(signup))
        .route("/users", get(list_users))
        .route("/users/me", get(current_user))
        .route("/users/{username}", delete(delete_user))
        .route("/users/{username}/plan", put(set_user_plan))
        .route("/databases", get(list_databases).post(create_database))
        .route("/databases/{id}", get(get_database).delete(delete_database))
        .route("/databases/{id}/execute", post(execute_query))
        .route("/databases/{id}/query", post(run_query))
        .route("/setup", post(setup_database))
        .route("/rate-limit", get(rate_limit_info))
        .route("/analytics", get(analytics_handler))
        .with_state(state)
}

fn check_rate_limit(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    state.rate_limiter.check(&ip).map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    Ok(())
}

async fn plan_for(state: &AppState, username: &str, user_type: &str) -> Plan {
    if user_type == "admin" {
        Plan::Enterprise
    } else {
        state.user_store.get_user(username).await.map(|u| Plan::from_str(&u.plan)).unwrap_or(Plan::Free)
    }
}

async fn check_user_rate_limit(state: &AppState, username: &str, user_type: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let plan = plan_for(state, username, user_type).await;
    state.rate_limiter.check_with_limit(username, plan.max_queries_per_minute())
        .map_err(|_| (StatusCode::TOO_MANY_REQUESTS, Json(ErrorResponse { error: "Query rate limit exceeded for your plan".into(), code: "RATE_LIMIT".into() })))?;
    Ok(())
}

async fn health_check(
    state: State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": Utc::now().to_rfc3339(),
        "data_dir": std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
        "auth_db_path": state.user_store.file_path(),
        "user_count": state.user_store.list_users().await.len()
    }))
}

async fn login(
    state: State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _ = check_rate_limit(&state, &headers);

    let config = Config::load().expect("Failed to load config");

    let admin_user = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let admin_pass = std::env::var("ADMIN_PASSWORD").expect("ADMIN_PASSWORD must be set in .env");

    if payload.username == admin_user && payload.password == admin_pass {
        let token = create_token(&payload.username, "admin", &config.jwt_secret, config.jwt_expiry_hours)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
        return Ok(Json(serde_json::json!({"token": token, "user": {"username": payload.username, "type": "admin", "plan": Plan::Enterprise.as_str()}})));
    }

    if let Ok(user) = state.user_store.verify_password(&payload.username, &payload.password).await {
        let token = create_token(&user.username, "user", &config.jwt_secret, config.jwt_expiry_hours)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
        return Ok(Json(serde_json::json!({"token": token, "user": {"username": user.username, "id": user.id, "type": "user", "plan": user.plan}})));
    }

    Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Invalid credentials", "code": "AUTH_FAILED"}))))
}

async fn google_token(
    Json(payload): Json<GoogleTokenRequest>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = Config::load().expect("Failed to load config");
    let jwt_token = create_token(&payload.email, "user", &config.jwt_secret, config.jwt_expiry_hours)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "JWT_ERROR".into() })))?;
    Ok(Json(UserResponse { id: generate_api_key(), email: payload.email, name: payload.name, token: jwt_token }))
}

async fn signup(
    state: State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateUserRequest>,
) -> Result<Json<UserInfo>, (StatusCode, Json<ErrorResponse>)> {
    authenticate_admin(&headers)?;
    let user = state.user_store.create_user(&payload.username, &payload.password).await
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e, code: "USER_ERROR".into() })))?;
    tracing::info!("Created user: {}", user.username);
    Ok(Json(UserInfo { id: user.id, username: user.username, plan: user.plan, created_at: user.created_at }))
}

async fn list_users(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ClientUserInfo>>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    if claims.typ != "admin" {
        return Err((StatusCode::FORBIDDEN, Json(ErrorResponse { error: "Admin only".into(), code: "FORBIDDEN".into() })));
    }
    let mut result: Vec<ClientUserInfo> = state.user_store.list_users().await.iter().map(|u| {
        let db_count = state.db_manager.list_databases(Some(&u.username)).len();
        ClientUserInfo { id: u.id.clone(), username: u.username.clone(), plan: u.plan.clone(), created_at: u.created_at.clone(), database_count: db_count }
    }).collect();
    result.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(Json(result))
}

async fn current_user(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    let plan = plan_for(&state, &claims.sub, &claims.typ).await;
    let db_count = if claims.typ == "admin" {
        state.db_manager.list_databases(None).len()
    } else {
        state.db_manager.list_databases(Some(&claims.sub)).len()
    };
    Ok(Json(serde_json::json!({
        "username": claims.sub,
        "type": claims.typ,
        "plan": plan.as_str(),
        "database_count": db_count,
        "max_databases": plan.max_databases()
    })))
}

async fn delete_user(
    state: State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    authenticate_admin(&headers)?;
    if is_admin(&username) {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Cannot delete admin".into(), code: "USER_ERROR".into() })));
    }
    state.user_store.delete_user(&username).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e, code: "USER_ERROR".into() })))?;
    let dbs = state.db_manager.list_databases(Some(&username));
    for (id, _) in dbs {
        let _ = state.db_manager.delete_database(&id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn set_user_plan(
    state: State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
    Json(payload): Json<PlanUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authenticate_admin(&headers)?;
    if is_admin(&username) {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Cannot change admin plan".into(), code: "USER_ERROR".into() })));
    }
    let plan = Plan::from_str(&payload.plan);
    let user = state.user_store.set_plan(&username, plan.as_str()).await
        .map_err(|e| (StatusCode::NOT_FOUND, Json(ErrorResponse { error: e, code: "USER_ERROR".into() })))?;
    tracing::info!("Set plan {} for user {}", plan.as_str(), username);
    Ok(Json(serde_json::json!({"username": user.username, "plan": user.plan})))
}

async fn list_databases(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DatabaseResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    state.query_tracker.track_query(&claims.sub);
    check_user_rate_limit(&state, &claims.sub, &claims.typ).await?;
    let owner = if claims.typ == "admin" { None } else { Some(claims.sub.as_str()) };
    let databases = state.db_manager.list_databases(owner);
    Ok(Json(databases.into_iter().map(|(id, entry)| DatabaseResponse {
        id, name: entry.name, owner: entry.owner, created_at: entry.created_at,
    }).collect()))
}

async fn create_database(
    state: State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateDatabaseRequest>,
) -> Result<Json<DatabaseResponse>, (StatusCode, Json<ErrorResponse>)> {
    let _ = check_rate_limit(&state, &headers);
    let claims = authenticate(&headers)?;
    check_user_rate_limit(&state, &claims.sub, &claims.typ).await?;
    let owner = &claims.sub;

    let plan = plan_for(&state, owner, &claims.typ).await;
    let user_db_count = state.db_manager.list_databases(Some(owner)).len();
    let max_for_user = plan.max_databases();
    if user_db_count >= max_for_user {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: format!("Maximum databases ({}) reached for your {} plan", max_for_user, plan.as_str()),
            code: "LIMIT_REACHED".into(),
        })));
    }

    let (id, entry) = state.db_manager.create_database(&payload.name, owner).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "CREATE_ERROR".into() })))?;

    Ok(Json(DatabaseResponse { id, name: entry.name, owner: entry.owner, created_at: entry.created_at }))
}

async fn get_database(
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DatabaseResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    check_db_owner(&state, &id, &claims.sub, &claims.typ)?;
    let (_, entry) = state.db_manager.get_database(&id).await
        .map_err(|_| (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Database not found".into(), code: "NOT_FOUND".into() })))?;
    Ok(Json(DatabaseResponse { id, name: entry.name, owner: entry.owner, created_at: entry.created_at }))
}

async fn delete_database(
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    check_db_owner(&state, &id, &claims.sub, &claims.typ)?;
    state.db_manager.delete_database(&id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "DELETE_ERROR".into() })))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn execute_query(
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    state.query_tracker.track_query(&claims.sub);
    check_user_rate_limit(&state, &claims.sub, &claims.typ).await?;
    check_db_owner(&state, &id, &claims.sub, &claims.typ)?;
    let result = state.db_manager.execute(&id, &payload.sql).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "EXECUTE_ERROR".into() })))?;
    Ok(Json(ExecuteResponse { success: true, message: result }))
}

async fn run_query(
    state: State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<ExecuteRequest>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    state.query_tracker.track_query(&claims.sub);
    check_user_rate_limit(&state, &claims.sub, &claims.typ).await?;
    check_db_owner(&state, &id, &claims.sub, &claims.typ)?;
    let rows = state.db_manager.query(&id, &payload.sql).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "QUERY_ERROR".into() })))?;
    let columns = if !rows.is_empty() { (0..rows[0].len()).map(|i| format!("column_{}", i)).collect() } else { vec![] };
    Ok(Json(QueryResponse { columns, rows }))
}

async fn setup_database(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SetupResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    let owner = &claims.sub;

    let existing = state.db_manager.list_databases(Some(owner));
    let (db_id, db_name) = if let Some((id, entry)) = existing.first() {
        (id.clone(), entry.name.clone())
    } else {
        let name = format!("{}-hub", owner);
        let (id, entry) = state.db_manager.create_database(&name, owner).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "SETUP_ERROR".into() })))?;
        (id, entry.name)
    };

    let schema = vec![
        "CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, description TEXT, status TEXT DEFAULT 'active', created_at TEXT DEFAULT (datetime('now')))",
        "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY AUTOINCREMENT, project_id INTEGER, title TEXT NOT NULL, description TEXT, status TEXT DEFAULT 'pending', created_at TEXT DEFAULT (datetime('now')))",
    ];
    for sql in &schema {
        state.db_manager.execute(&db_id, sql).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string(), code: "SETUP_ERROR".into() })))?;
    }

    let seeded = state.db_manager.query(&db_id, "SELECT COUNT(*) as c FROM projects").await
        .map(|rows| rows.first().and_then(|r| r.first()).map(|c| c != "0").unwrap_or(false))
        .unwrap_or(false);

    if !seeded {
        let _ = state.db_manager.execute(&db_id,
            "INSERT INTO projects (name, description) VALUES ('My Project', 'Auto-created on setup')"
        ).await;
    }

    Ok(Json(SetupResponse {
        database_id: db_id,
        database_name: db_name,
        schema: schema.iter().map(|s| s.to_string()).collect(),
        seeded: true,
    }))
}

async fn rate_limit_info(
    state: State<AppState>,
) -> Json<RateLimitInfo> {
    Json(RateLimitInfo {
        remaining: state.rate_limiter.max_requests(),
        limit: state.rate_limiter.max_requests(),
        window_secs: state.rate_limiter.window_secs(),
    })
}

fn authenticate(headers: &HeaderMap) -> Result<Claims, (StatusCode, Json<ErrorResponse>)> {
    let config = Config::load().expect("Failed to load config");
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Missing authorization header".into(), code: "UNAUTHORIZED".into() })))?;
    let token = extract_token_from_header(auth_header)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Invalid authorization format".into(), code: "UNAUTHORIZED".into() })))?;
    verify_token(&token, &config.jwt_secret)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ErrorResponse { error: "Invalid or expired token".into(), code: "UNAUTHORIZED".into() })))
}

fn authenticate_admin(headers: &HeaderMap) -> Result<Claims, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(headers)?;
    if claims.typ != "admin" {
        return Err((StatusCode::FORBIDDEN, Json(ErrorResponse { error: "Admin access required".into(), code: "FORBIDDEN".into() })));
    }
    Ok(claims)
}

fn check_db_owner(state: &AppState, db_id: &str, user: &str, user_type: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if user_type == "admin" { return Ok(()); }
    match state.db_manager.get_db_owner(db_id) {
        Some(owner) if owner == user => Ok(()),
        Some(_) => Err((StatusCode::FORBIDDEN, Json(ErrorResponse { error: "Not your database".into(), code: "FORBIDDEN".into() }))),
        None => Err((StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Database not found".into(), code: "NOT_FOUND".into() }))),
    }
}

async fn analytics_handler(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AnalyticsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = authenticate(&headers)?;
    let is_admin = claims.typ == "admin";

    let (total_queries, database_count, user_count) = if is_admin {
        let dbs = state.db_manager.list_databases(None);
        let users = state.user_store.list_users().await;
        (state.query_tracker.get_total_all(), dbs.len(), users.len())
    } else {
        let dbs = state.db_manager.list_databases(Some(&claims.sub));
        (state.query_tracker.get_total(&claims.sub), dbs.len(), 0)
    };

    let volume_raw = if is_admin {
        state.query_tracker.get_volume_all()
    } else {
        state.query_tracker.get_volume(&claims.sub)
    };

    let volume: Vec<VolumePoint> = volume_raw.into_iter().map(|(ts, count)| VolumePoint { timestamp: ts, count }).collect();

    Ok(Json(AnalyticsResponse { total_queries, database_count, user_count, volume }))
}

#[derive(Debug, serde::Deserialize)]
pub struct GoogleTokenRequest {
    pub email: String,
    pub name: String,
}
