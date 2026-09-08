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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn admits_up_to_default_max() {
        let rl = RateLimiter::new(3, 60);
        for i in 0..3 {
            assert_eq!(rl.check("ip-1"), Ok(2 - i), "request {i} should pass");
        }
        // 4th is over the limit
        assert_eq!(rl.check("ip-1"), Err(0));
    }

    #[test]
    fn tracks_keys_independently() {
        let rl = RateLimiter::new(1, 60);
        assert!(rl.check("a").is_ok());
        assert_eq!(rl.check("a"), Err(0));
        assert!(rl.check("b").is_ok());
    }

    #[test]
    fn custom_limit_overrides_default() {
        let rl = RateLimiter::new(100, 60);
        assert!(rl.check_with_limit("ip-1", 2).is_ok());
        assert!(rl.check_with_limit("ip-1", 2).is_ok());
        assert_eq!(rl.check_with_limit("ip-1", 2), Err(0));
    }

    #[test]
    fn window_resets_after_elapse() {
        let rl = RateLimiter::new(1, 3600); // huge window; won't reset naturally
        let _ = rl.check("k");
        assert_eq!(rl.check("k"), Err(0));
        // Simulate a reset by directly manipulating the internal state.
        {
            let mut entry = rl.store.get_mut("k").unwrap();
            entry.window_start = Instant::now()
                .checked_sub(Duration::from_secs(3601))
                .unwrap();
        }
        assert!(
            rl.check("k").is_ok(),
            "after window elapsed, request passes"
        );
    }

    #[test]
    fn max_and_window_accessors() {
        let rl = RateLimiter::new(5, 30);
        assert_eq!(rl.max_requests(), 5);
        assert_eq!(rl.window_secs(), 30);
    }
}
