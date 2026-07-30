use chrono::Utc;
use dashmap::DashMap;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct QueryTracker {
    volume: Arc<DashMap<String, Vec<(i64, u64)>>>,
    totals: Arc<DashMap<String, u64>>,
}

impl QueryTracker {
    pub fn new() -> Self {
        Self {
            volume: Arc::new(DashMap::new()),
            totals: Arc::new(DashMap::new()),
        }
    }

    pub fn track_query(&self, username: &str) {
        let now_minute = Utc::now().timestamp() / 60 * 60;

        let mut entry = self.volume.entry(username.to_string()).or_default();
        if let Some(last) = entry.last_mut() {
            if last.0 == now_minute {
                last.1 += 1;
            } else {
                if entry.len() > 1440 {
                    entry.remove(0);
                }
                entry.push((now_minute, 1));
            }
        } else {
            entry.push((now_minute, 1));
        }

        *self.totals.entry(username.to_string()).or_insert(0) += 1;
    }

    pub fn get_total(&self, username: &str) -> u64 {
        self.totals.get(username).map(|e| *e).unwrap_or(0)
    }

    pub fn get_total_all(&self) -> u64 {
        self.totals.iter().map(|e| *e.value()).sum()
    }

    pub fn get_volume(&self, username: &str) -> Vec<(i64, u64)> {
        self.volume.get(username).map(|e| e.value().clone()).unwrap_or_default()
    }

    pub fn get_volume_all(&self) -> Vec<(i64, u64)> {
        let mut merged: HashMap<i64, u64> = HashMap::new();
        for entry in self.volume.iter() {
            for (ts, count) in entry.value() {
                *merged.entry(*ts).or_insert(0) += count;
            }
        }
        let mut result: Vec<(i64, u64)> = merged.into_iter().collect();
        result.sort_by_key(|(ts, _)| *ts);
        result
    }

    pub fn list_user_totals(&self) -> Vec<(String, u64)> {
        self.totals.iter().map(|e| (e.key().clone(), *e.value())).collect()
    }
}

#[derive(Serialize)]
pub struct AnalyticsResponse {
    pub total_queries: u64,
    pub database_count: usize,
    pub user_count: usize,
    pub volume: Vec<VolumePoint>,
}

#[derive(Serialize)]
pub struct VolumePoint {
    pub timestamp: i64,
    pub count: u64,
}
