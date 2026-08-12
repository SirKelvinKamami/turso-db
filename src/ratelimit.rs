use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct RateLimiter {
    store: Arc<DashMap<String, RateLimitState>>,
    default_max: u64,
    window_secs: u64,
}

struct RateLimitState {
    count: u64,
    window_start: Instant,
    max: u64,
}

impl RateLimiter {
    pub fn new(default_max: u64, window_secs: u64) -> Self {
        Self {
            store: Arc::new(DashMap::new()),
            default_max,
            window_secs,
        }
    }

    pub fn check(&self, key: &str) -> Result<u64, u64> {
        self.check_with_limit(key, self.default_max)
    }

    pub fn check_with_limit(&self, key: &str, max: u64) -> Result<u64, u64> {
        let now = Instant::now();
        let mut entry = self.store.entry(key.to_string()).or_insert(RateLimitState {
            count: 0,
            window_start: now,
            max,
        });

        let elapsed = now.duration_since(entry.window_start).as_secs();
        if elapsed > self.window_secs {
            entry.count = 1;
            entry.window_start = now;
            entry.max = max;
            Ok(max.saturating_sub(1))
        } else if entry.count >= entry.max {
            Err(0)
        } else {
            entry.count += 1;
            Ok(entry.max.saturating_sub(entry.count))
        }
    }

    pub fn max_requests(&self) -> u64 {
        self.default_max
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }
}
