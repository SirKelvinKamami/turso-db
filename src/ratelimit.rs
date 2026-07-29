use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct RateLimiter {
    store: Arc<DashMap<String, RateLimitState>>,
    max_requests: u64,
    window_secs: u64,
}

struct RateLimitState {
    count: u64,
    window_start: Instant,
}

impl RateLimiter {
    pub fn new(max_requests: u64, window_secs: u64) -> Self {
        Self {
            store: Arc::new(DashMap::new()),
            max_requests,
            window_secs,
        }
    }

    pub fn check(&self, key: &str) -> Result<u64, u64> {
        let now = Instant::now();
        let mut entry = self.store.entry(key.to_string()).or_insert(RateLimitState {
            count: 0,
            window_start: now,
        });

        let elapsed = now.duration_since(entry.window_start).as_secs();
        if elapsed > self.window_secs {
            entry.count = 1;
            entry.window_start = now;
            Ok(self.max_requests - 1)
        } else if entry.count >= self.max_requests {
            Err(0)
        } else {
            entry.count += 1;
            Ok(self.max_requests - entry.count)
        }
    }

    pub fn max_requests(&self) -> u64 {
        self.max_requests
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }
}
