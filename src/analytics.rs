use chrono::Utc;
use dashmap::{DashMap, DashSet};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::supabase::Supabase;

const ANALYTICS_TABLE: &str = "turso_analytics";
const MAX_VOLUME_POINTS: usize = 1440;
pub const FLUSH_INTERVAL_SECS: u64 = 30;

#[derive(Clone)]
pub struct QueryTracker {
    volume: Arc<DashMap<String, Vec<(i64, u64)>>>,
    totals: Arc<DashMap<String, u64>>,
    dirty: Arc<DashSet<String>>,
    supabase: Option<Supabase>,
}

impl QueryTracker {
    pub fn new(supabase: Option<Supabase>) -> Self {
        Self {
            volume: Arc::new(DashMap::new()),
            totals: Arc::new(DashMap::new()),
            dirty: Arc::new(DashSet::new()),
            supabase,
        }
    }

    pub async fn load_from_supabase(&self) {
        let sb = match &self.supabase {
            Some(sb) => sb,
            None => return,
        };
        match sb.rows(ANALYTICS_TABLE, "").await {
            Ok(rows) => {
                for row in rows {
                    if let Some(username) = row.get("username").and_then(|v| v.as_str()) {
                        let total = row.get("total_queries").and_then(|v| v.as_u64()).unwrap_or(0);
                        *self.totals.entry(username.to_string()).or_insert(0) = total;
                        if let Some(vol) = row.get("volume").and_then(|v| v.as_array()) {
                            let points: Vec<(i64, u64)> = vol.iter().filter_map(|p| {
                                let arr = p.as_array()?;
                                Some((arr.first()?.as_i64()?, arr.get(1)?.as_u64()?))
                            }).collect();
                            self.volume.insert(username.to_string(), points);
                        }
                    }
                }
                tracing::info!("Loaded analytics for {} user(s) from Supabase", rows.len());
            }
            Err(e) => tracing::error!("Failed to load analytics from Supabase: {}", e),
        }
    }

    pub fn track_query(&self, username: &str) {
        let now_minute = Utc::now().timestamp() / 60 * 60;

        let mut entry = self.volume.entry(username.to_string()).or_default();
        if let Some(last) = entry.last_mut() {
            if last.0 == now_minute {
                last.1 += 1;
            } else {
                if entry.len() > MAX_VOLUME_POINTS {
                    entry.remove(0);
                }
                entry.push((now_minute, 1));
            }
        } else {
            entry.push((now_minute, 1));
        }

        *self.totals.entry(username.to_string()).or_insert(0) += 1;
        self.dirty.insert(username.to_string());
    }

    pub async fn flush(&self) -> usize {
        if self.dirty.is_empty() {
            return 0;
        }
        let sb = match &self.supabase {
            Some(sb) => sb,
            None => {
                self.dirty.clear();
                return 0;
            }
        };
        let usernames: Vec<String> = self.dirty.iter().map(|e| e.key().clone()).collect();
        let mut rows = Vec::new();
        for username in &usernames {
            let total = self.totals.get(username).map(|e| *e).unwrap_or(0);
            let volume = self.volume.get(username).map(|e| e.value().clone()).unwrap_or_default();
            let vol_json: Vec<Vec<i64>> = volume
                .iter()
                .map(|(ts, count)| vec![*ts, *count as i64])
                .collect();
            rows.push(json!({
                "username": username,
                "total_queries": total as i64,
                "volume": vol_json,
                "updated_at": Utc::now().to_rfc3339(),
            }));
        }
        let row_count = rows.len();
        match sb.upsert(ANALYTICS_TABLE, json!(rows)).await {
            Ok(()) => {
                for username in &usernames {
                    self.dirty.remove(username);
                }
                row_count
            }
            Err(e) => {
                tracing::error!("Failed to flush analytics to Supabase: {}", e);
                0
            }
        }
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
