use axum::http::StatusCode;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub typ: String,
    pub exp: usize,
}

pub fn create_token(
    user_id: &str,
    user_type: &str,
    secret: &str,
    expiry_hours: u64,
) -> Result<String, Box<dyn std::error::Error>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip() {
        let secret = "test-secret-please-ignore".to_string();
        let token = create_token("user-1", "user", &secret, 24).unwrap();
        let claims = verify_token(&token, &secret).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.typ, "user");
    }

    #[test]
    fn token_rejects_wrong_secret() {
        let token = create_token("user-1", "user", "secret-a", 24).unwrap();
        assert!(verify_token(&token, "secret-b").is_err());
    }

    #[test]
    fn token_rejects_garbage() {
        assert!(verify_token("not-a-token", "secret").is_err());
        assert!(verify_token("", "secret").is_err());
    }

    #[test]
    fn token_expires_after_duration() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        let expired = Claims {
            sub: "user-1".into(),
            typ: "user".into(),
            exp: (chrono::Utc::now() - chrono::Duration::hours(2)).timestamp() as usize,
        };
        let token = encode(
            &Header::default(),
            &expired,
            &EncodingKey::from_secret(b"s"),
        )
        .unwrap();
        assert!(verify_token(&token, "s").is_err());
    }

    #[test]
    fn extracts_bearer_token() {
        assert_eq!(extract_token_from_header("Bearer abc").unwrap(), "abc");
        assert_eq!(
            extract_token_from_header("bearer abc").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            extract_token_from_header("abc").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            extract_token_from_header("").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn admin_matches_case_insensitively() {
        // Default env: admin
        assert!(is_admin("admin"));
        assert!(is_admin("ADMIN"));
        assert!(is_admin("Admin"));
        assert!(!is_admin("not-admin"));
    }

    #[test]
    fn api_keys_are_unique() {
        assert_ne!(generate_api_key(), generate_api_key());
        assert!(generate_api_key().len() >= 32);
    }
}
