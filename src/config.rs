use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub bind_address: String,
    pub data_dir: String,
    pub jwt_secret: String,
    pub jwt_expiry_hours: u64,
    pub max_databases: usize,
    pub max_queries_per_minute: u64,
    pub encryption_key: Option<String>,
    pub seed_users: Vec<(String, String)>,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        // Load .env file if present
        let _ = dotenvy::from_path(Path::new(".env"));

        Ok(Self {
            bind_address: std::env::var("BIND_ADDRESS")
                .or_else(|_| std::env::var("PORT").map(|p| format!("0.0.0.0:{}", p)))
                .unwrap_or_else(|_| "0.0.0.0:3000".to_string()),
            data_dir: std::env::var("DATA_DIR")
                .unwrap_or_else(|_| "./data".to_string()),
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "change-me-in-production".to_string()),
            jwt_expiry_hours: std::env::var("JWT_EXPIRY_HOURS")
                .unwrap_or_else(|_| "24".to_string())
                .parse()?,
            max_databases: std::env::var("MAX_DATABASES")
                .unwrap_or_else(|_| "100".to_string())
                .parse()?,
            max_queries_per_minute: std::env::var("MAX_QUERIES_PER_MINUTE")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            encryption_key: std::env::var("ENCRYPTION_KEY").ok(),
            seed_users: {
                let raw = std::env::var("SEED_USERS").unwrap_or_default();
                raw.split(',')
                    .filter(|s| !s.is_empty())
                    .filter_map(|pair| {
                        let parts: Vec<&str> = pair.split(':').collect();
                        if parts.len() == 2 {
                            Some((parts[0].to_string(), parts[1].to_string()))
                        } else {
                            tracing::warn!("Invalid SEED_USERS entry: {}", pair);
                            None
                        }
                    })
                    .collect()
            },
        })
    }
}
