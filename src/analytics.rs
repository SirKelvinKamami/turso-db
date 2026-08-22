use chrono::Utc;
use dashmap::{DashMap, DashSet};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::supabase::Supabase;

const ANALYTICS_TABLE: &str = "turso_analytics";
pub const FLUSH_INTERVAL_SECS: u64 = 30;
const HARD_CAP_POINTS: usize = 20160;

#[derive(Clone)]
pub struct QueryTracker {
    volume: Arc<DashMap<String, Vec<(i64, u64)>>>,
    totals: Arc<DashMap<String, u64>>,
    dirty: Arc<DashSet<String>>,
    supabase: Option<Supabase>,
    retention_secs: i64,
}

fn default_retention() -> i64 {
    let hours: u64 = std::env::var("ANALYTICS_RETENTION_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(168);
    (hours * 3600) as i64
}

fn prune_points(points: &mut Vec<(i64, u64)>, cutoff: i64) {
    if points.first().map(|(ts, _)| *ts < cutoff).unwrap_or(false) {
        points.retain(|(ts, _)| *ts >= cutoff);
    }
    while points.len() > HARD_CAP_POINTS {
        points.remove(0);
    }
}

fn json_to_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

impl QueryTracker {
    pub fn new(supabase: Option<Supabase>) -> Self {
        Self {
            volume: Arc::new(DashMap::new()),
            totals: Arc::new(DashMap::new()),
            dirty: Arc::new(DashSet::new()),
            supabase,
            retention_secs: default_retention(),
        }
    }

    pub async fn load_from_supabase(&self) {
        let sb = match &self.supabase {
            Some(sb) => sb,
            None => return,
        };
        let cutoff = Utc::now().timestamp() - self.retention_secs;
        let mut rows: Vec<Value> = Vec::new();
        for attempt in 1..=5u32 {
            match sb.rows(ANALYTICS_TABLE, "").await {
                Ok(r) => {
                    rows = r;
                    break;
                }
                Err(e) => {
                    tracing::warn!("Analytics load attempt {}/{} failed: {}", attempt, 5, e);
                    if attempt < 5 {
                        tokio::time::sleep(std::time::Duration::from_secs(5 * attempt as u64)).await;
                    }
                }
            }
        }
        let row_count = rows.len();
        for row in rows {
            if let Some(username) = row.get("username").and_then(|v| v.as_str()) {
                let total = row.get("total_queries").and_then(json_to_u64).unwrap_or(0);
                *self.totals.entry(username.to_string()).or_insert(0) = total;
                if let Some(vol) = row.get("volume").and_then(|v| v.as_array()) {
                    let mut points: Vec<(i64, u64)> = vol.iter().filter_map(|p| {
                        let arr = p.as_array()?;
                        Some((arr.first()?.as_i64()?, arr.get(1)?.as_u64()?))
                    }).collect();
                    prune_points(&mut points, cutoff);
                    self.volume.insert(username.to_string(), points);
                }
            }
        }
        tracing::info!("Loaded analytics for {} user(s) from Supabase", row_count);
    }

    pub fn track_query(&self, username: &str) {
        let now_minute = Utc::now().timestamp() / 60 * 60;

        let mut entry = self.volume.entry(username.to_string()).or_default();
        let cutoff = Utc::now().timestamp() - self.retention_secs;
        if let Some(last) = entry.last_mut() {
            if last.0 == now_minute {
                last.1 += 1;
            } else {
                entry.push((now_minute, 1));
            }
        } else {
            entry.push((now_minute, 1));
        }
        prune_points(entry.value_mut(), cutoff);

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
        let cutoff = Utc::now().timestamp() - self.retention_secs;
        let mut rows = Vec::new();
        for username in &usernames {
            let total = self.totals.get(username).map(|e| *e).unwrap_or(0);
            let volume = {
                let mut entry = self.volume.entry(username.clone()).or_default();
                prune_points(entry.value_mut(), cutoff);
                entry.value().clone()
            };
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

#[derive(Serialize, Clone)]
pub struct UserTotalsEntry {
    pub username: String,
    pub total_queries: u64,
}

#[derive(Serialize)]
pub struct AnalyticsResponse {
    pub total_queries: u64,
    pub database_count: usize,
    pub user_count: usize,
    pub volume: Vec<VolumePoint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub per_user: Vec<UserTotalsEntry>,
}

#[derive(Serialize)]
pub struct VolumePoint {
    pub timestamp: i64,
    pub count: u64,
}
