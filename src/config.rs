use serde::Deserialize;
use std::path::Path;

/// Embedded-replica sync settings (opt-in). When `enabled` and `hub_url` are set,
/// databases are opened through Turso's sync engine instead of as plain local files so
/// they converge (bidirectionally, last-writer-wins) with a self-hosted libsql server.
#[derive(Debug, Clone, Default)]
pub struct SyncConfig {
    /// Master kill switch. False keeps every database plain-local (no sync IO, no
    /// supervisor task, zero change to existing behavior).
    pub enabled: bool,
    /// Base URL of the libsql hub server (e.g. `libsql://hub.example.com` or
    /// `http://127.0.0.1:8080`). The per-database remote is `{hub_url}/{db-name}`.
    pub hub_url: Option<String>,
    /// Token sent as `Authorization: Bearer` to the hub for every sync request.
    /// Deliberately never persisted next to database manifests/flies.
    pub hub_token: Option<String>,
    /// Supervisor idle cadence (milliseconds). Also used as the sync long-poll timeout
    /// so a quiet hub returns within roughly one cadence.
    pub poll_ms: u64,
}

impl SyncConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("SYNC_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let poll_ms = std::env::var("SYNC_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000)
            .max(250);
        Self {
            enabled,
            hub_url: std::env::var("SYNC_HUB_URL").ok(),
            hub_token: std::env::var("SYNC_HUB_TOKEN").ok(),
            poll_ms,
        }
    }
}

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
    /// Loaded from the environment, not from any config file; keep out of serde.
    #[serde(skip)]
    pub sync: SyncConfig,
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
            sync: SyncConfig::from_env(),
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
