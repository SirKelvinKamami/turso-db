use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct DatabaseResponse {
    pub id: String,
    pub name: String,
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
