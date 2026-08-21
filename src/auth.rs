use axum::http::StatusCode;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub typ: String,
    pub exp: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token: String,
}

pub fn create_token(user_id: &str, user_type: &str, secret: &str, expiry_hours: u64) -> Result<String, Box<dyn std::error::Error>> {
    let claims = Claims {
        sub: user_id.to_string(),
        typ: user_type.to_string(),
        exp: chrono::Utc::now()
            .checked_add_signed(chrono::Duration::hours(expiry_hours as i64))
            .expect("valid timestamp")
            .timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> Result<Claims, StatusCode> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| StatusCode::UNAUTHORIZED)
}

pub fn extract_token_from_header(auth_header: &str) -> Result<String, StatusCode> {
    auth_header
        .strip_prefix("Bearer ")
        .map(|s| s.to_string())
        .ok_or(StatusCode::UNAUTHORIZED)
}

pub fn generate_api_key() -> String {
    Uuid::new_v4().to_string()
}

pub fn is_admin(username: &str) -> bool {
    let configured = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    username.eq_ignore_ascii_case(&configured)
}
