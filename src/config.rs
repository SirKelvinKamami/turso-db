use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub bind_address: String,
    pub data_dir: String,
    pub jwt_secret: String,
    pub jwt_expiry_hours: u64,
    #[allow(dead_code)]
    pub max_databases: usize,
    pub max_queries_per_minute: u64,
    #[allow(dead_code)]
    pub encryption_key: Option<String>,
    pub google_client_id: String,
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
            data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
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
            google_client_id: std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            seed_users: parse_seed_users(&std::env::var("SEED_USERS").unwrap_or_default()),
        })
    }
}

fn parse_seed_users(raw: &str) -> Vec<(String, String)> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seed_users() {
        let seeds = parse_seed_users("alice:pass1,bob:pass2");
        assert_eq!(
            seeds,
            vec![
                ("alice".to_string(), "pass1".to_string()),
                ("bob".to_string(), "pass2".to_string()),
            ]
        );
    }

    #[test]
    fn empty_or_malformed_seed_users() {
        assert!(parse_seed_users("").is_empty());
        assert!(parse_seed_users(",,,").is_empty());
        assert!(parse_seed_users("no-colon-here").is_empty());
        assert!(parse_seed_users("a:b:c").is_empty());
        // Malformed entries are dropped, valid ones kept.
        assert_eq!(
            parse_seed_users("good:pw,badentry"),
            vec![("good".to_string(), "pw".to_string())]
        );
    }
}
