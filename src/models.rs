use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct DatabaseResponse {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub sql: String,
}

#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct GoogleCallback {
    pub code: String,
    pub state: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub plan: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ClientUserInfo {
    pub id: String,
    pub username: String,
    pub plan: String,
    pub created_at: String,
    pub database_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct PlanUpdateRequest {
    pub plan: String,
}

#[derive(Debug, Serialize)]
pub struct SetupResponse {
    pub database_id: String,
    pub database_name: String,
    pub schema: Vec<String>,
    pub seeded: bool,
}

#[derive(Debug, Serialize)]
pub struct RateLimitInfo {
    pub remaining: u64,
    pub limit: u64,
    pub window_secs: u64,
}
