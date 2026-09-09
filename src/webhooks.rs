use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::supabase::Supabase;

pub const EVENT_WRITE: &str = "write";
const ALLOWED_EVENTS: [&str; 2] = [EVENT_WRITE, "*"];
const SIGNATURE_PREFIX: &str = "sha256=";
const SUPABASE_TABLE: &str = "turso_webhooks";
const DEFAULT_BACKOFF: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];
const PENDING_TTL_DEFAULT_SECS: u64 = 7 * 24 * 60 * 60; // 7 days
const PENDING_MAX_ATTEMPTS_DEFAULT: u32 = 10080; // one retry tick per 60s over 7 days

/// Per-webhook retry policy. `max_attempts` is the total number of delivery attempts in
/// the immediate in-memory burst (including the first); `backoff_ms` lists the delays
/// applied before each subsequent attempt. If the list is shorter than `max_attempts - 1`,
/// extra attempts reuse the request timeout spacing (no extra delay) — or simply cap the
/// burst by providing exactly `max_attempts - 1` entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: Vec<u64>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: (DEFAULT_BACKOFF.len() as u32) + 1,
            backoff_ms: DEFAULT_BACKOFF
                .iter()
                .map(|d| d.as_millis() as u64)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default = "default_events")]
    pub events: Vec<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
    pub created_at: String,
}

fn default_events() -> Vec<String> {
    vec![EVENT_WRITE.to_string()]
}

impl Webhook {
    pub fn matches(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == "*" || e == event)
    }

    /// Delays to sleep between in-memory delivery attempts for this hook.
    pub fn in_memory_delays(&self) -> Vec<Duration> {
        let policy = self.retry.clone().unwrap_or_default();
        let extra = policy.max_attempts.saturating_sub(1) as usize;
        policy
            .backoff_ms
            .iter()
            .take(extra)
            .map(|ms| Duration::from_millis(*ms))
            .collect()
    }

    /// Total attempts made in a single in-memory burst before failing over to the durable
    /// pending queue.
    pub fn burst_attempts(&self) -> u32 {
        1 + self.in_memory_delays().len() as u32
    }
}

/// A single delivery that has not yet been confirmed by its receiver. Persists across
/// restarts so a crash between retry attempts does not drop the webhook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDelivery {
    pub id: String,
    pub db_id: String,
    pub event: String,
    pub hook: Webhook,
    pub attempts: u32,
    pub payload: Vec<u8>,
    /// When the delivery first became pending (UTC RFC 3339). Used to bound retries via
    /// a TTL so permanently-dead receivers do not accumulate files forever.
    #[serde(default = "clock_now_rfc3339")]
    pub created_at: String,
}

fn clock_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Columnar snapshot of a statement's affected rows (stringified like query output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowFrame {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// True change-frame for one write statement, captured around the write itself:
/// `before` holds the rows matched by the statement's WHERE clause (for UPDATE/DELETE)
/// and `after` holds the rows produced by `RETURNING rowid, *` (for INSERT/UPDATE).
/// `None` fields mean the snapshot could not be produced (e.g. WITHOUT ROWID tables).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatementFrame {
    pub op: Option<String>,
    pub table: Option<String>,
    pub before: Option<RowFrame>,
    pub after: Option<RowFrame>,
}

impl StatementFrame {
    /// Shallow non-empty check so payloads skip empty frames.
    pub fn has_rows(&self) -> bool {
        self.before.as_ref().is_some_and(|f| !f.rows.is_empty())
            || self.after.as_ref().is_some_and(|f| !f.rows.is_empty())
    }
}

#[derive(Clone)]
pub struct WebhookStore {
    path: String,
    pending_dir: String,
    hooks: Arc<DashMap<String, Vec<Webhook>>>,
    client: reqwest::Client,
    supabase: Option<Supabase>,
}

impl WebhookStore {
    pub fn new(path: &str, supabase: Option<Supabase>) -> Self {
        let data_dir = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string());
        let pending_dir = std::path::Path::new(&data_dir)
            .join("pending_deliveries")
            .to_string_lossy()
            .into_owned();
        std::fs::create_dir_all(&pending_dir).ok();
        let store = Self {
            path: path.to_string(),
            pending_dir,
            hooks: Arc::new(DashMap::new()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            supabase,
        };
        store.load();
        store
    }

    fn load(&self) {
        if Path::new(&self.path).exists() {
            self.load_file();
            return;
        }
        if self.supabase.is_some() {
            let store = self.clone();
            tokio::spawn(async move { store.load_from_supabase().await });
        }
    }

    fn load_file(&self) {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => {
                let data: serde_json::Value =
                    serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
                if let Some(map) = data.as_object() {
                    for (db_id, hooks) in map {
                        let list = match hooks {
                            serde_json::Value::Array(items) => items
                                .iter()
                                .filter_map(|h| serde_json::from_value::<Webhook>(h.clone()).ok())
                                .collect(),
                            _ => Vec::new(),
                        };
                        if !list.is_empty() {
                            self.hooks.insert(db_id.clone(), list);
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Failed to read webhooks file: {}", e),
        }
    }

    async fn load_from_supabase(&self) {
        let Some(sb) = &self.supabase else {
            return;
        };
        match sb.rows(SUPABASE_TABLE, "").await {
            Ok(rows) => {
                let mut changed = false;
                for row in rows {
                    let Some(db_id) = row.get("id").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let hooks = row.get("hooks").cloned().unwrap_or(Value::Null);
                    let list = parse_hooks_value(hooks);
                    if !list.is_empty() {
                        self.hooks.insert(db_id.to_string(), list);
                        changed = true;
                    }
                }
                if changed {
                    self.save_file();
                }
            }
            Err(e) => tracing::warn!("Failed to load webhooks from Supabase: {}", e),
        }
    }

    fn save(&self) {
        self.save_file();
        let Some(sb) = self.supabase.clone() else {
            return;
        };
        let rows: Vec<Value> = self
            .hooks
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.key(),
                    "hooks": serde_json::to_value(entry.value().clone()).unwrap_or_default(),
                })
            })
            .collect();
        let raw = serde_json::to_string(&Value::Array(rows)).unwrap_or_else(|_| "[]".to_string());
        tokio::spawn(async move {
            if let Ok(payload) = serde_json::from_str::<Value>(&raw)
                && let Err(e) = sb.upsert(SUPABASE_TABLE, payload).await
            {
                tracing::warn!("Supabase webhooks upsert failed: {}", e);
            }
        });
    }

    fn save_file(&self) {
        let map: serde_json::Map<String, serde_json::Value> = self
            .hooks
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    serde_json::to_value(entry.value().clone()).unwrap_or_default(),
                )
            })
            .collect();
        let raw = serde_json::to_string_pretty(&serde_json::Value::Object(map))
            .unwrap_or_else(|_| "{}".to_string());
        if let Err(e) = std::fs::write(&self.path, raw) {
            tracing::error!("Failed to save webhooks file: {}", e);
        }
    }

    pub fn list(&self, db_id: &str) -> Vec<Webhook> {
        self.hooks
            .get(db_id)
            .map(|hooks| hooks.clone())
            .unwrap_or_default()
    }

    pub fn add(
        &self,
        db_id: &str,
        url: &str,
        secret: Option<String>,
        events: Option<Vec<String>>,
        headers: Option<HashMap<String, String>>,
        retry: Option<RetryPolicy>,
    ) -> Result<Webhook, String> {
        validate_url(url)?;
        let events = events.unwrap_or_else(default_events);
        validate_events(&events)?;
        let headers = headers.unwrap_or_default();
        validate_headers(&headers)?;
        validate_retry(retry.as_ref())?;
        let hook = Webhook {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            secret: secret.unwrap_or_default(),
            events,
            headers,
            retry,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.hooks
            .entry(db_id.to_string())
            .or_default()
            .push(hook.clone());
        self.save();
        Ok(hook)
    }

    /// Partial update of an existing webhook. All fields validated before any mutation;
    /// a failing validation leaves the hook untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        db_id: &str,
        hook_id: &str,
        url: Option<&str>,
        secret: Option<Option<String>>,
        events: Option<&[String]>,
        headers: Option<&HashMap<String, String>>,
        retry: Option<Option<RetryPolicy>>,
    ) -> Result<Webhook, String> {
        let mut hooks = self
            .hooks
            .get_mut(db_id)
            .ok_or_else(|| "webhook not found".to_string())?;
        let Some(hook) = hooks.iter_mut().find(|h| h.id == hook_id) else {
            return Err("webhook not found".to_string());
        };
        if let Some(url) = url {
            validate_url(url)?;
        }
        if let Some(events) = events {
            validate_events(events)?;
        }
        if let Some(headers) = headers {
            validate_headers(headers)?;
        }
        if let Some(retry) = &retry {
            validate_retry(retry.as_ref())?;
        }
        if let Some(url) = url {
            hook.url = url.to_string();
        }
        if let Some(secret) = secret {
            hook.secret = secret.unwrap_or_default();
        }
        if let Some(events) = events {
            hook.events = events.to_vec();
        }
        if let Some(headers) = headers {
            hook.headers = headers.clone();
        }
        if let Some(retry) = retry {
            hook.retry = retry;
        }
        let updated = hook.clone();
        drop(hooks);
        self.save();
        Ok(updated)
    }

    pub fn remove(&self, db_id: &str, hook_id: &str) -> bool {
        let mut removed = false;
        if let Some(mut hooks) = self.hooks.get_mut(db_id) {
            let before = hooks.len();
            hooks.retain(|h| h.id != hook_id);
            removed = hooks.len() != before;
        }
        if removed {
            self.save();
            self.purge_pending_for(db_id, Some(hook_id));
        }
        removed
    }

    pub fn remove_all(&self, db_id: &str) {
        if self.hooks.remove(db_id).is_some() {
            self.save();
            self.purge_pending_for(db_id, None);
            if let Some(sb) = self.supabase.clone() {
                let did = db_id.to_string();
                tokio::spawn(async move {
                    if let Err(e) = sb.delete(SUPABASE_TABLE, &format!("id=eq.{}", did)).await {
                        tracing::warn!("Supabase webhooks delete failed: {}", e);
                    }
                });
            }
        }
    }

    /// Remove queued deliveries for a database (and optionally a specific webhook) so
    /// retries for deleted resources stop.
    fn purge_pending_for(&self, db_id: &str, hook_id: Option<&str>) {
        if let Ok(entries) = std::fs::read_dir(&self.pending_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let keep = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<PendingDelivery>(&raw).ok())
                    .map(|d| d.db_id != db_id || hook_id.is_some_and(|hid| d.hook.id == hid))
                    .unwrap_or(true);
                if !keep {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &self,
        db_id: &str,
        db_name: &str,
        owner: &str,
        event: &str,
        statements: Vec<String>,
        rows_affected: u64,
        frames: Vec<StatementFrame>,
    ) {
        let hooks = self.list(db_id);
        let receivers: Vec<Webhook> = hooks.into_iter().filter(|h| h.matches(event)).collect();
        let body = build_payload(
            db_id,
            db_name,
            owner,
            event,
            &statements,
            rows_affected,
            &frames,
        );
        let raw = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

        for hook in receivers {
            let store = self.clone();
            let body = raw.clone();
            let did = db_id.to_string();
            let ev = event.to_string();
            tokio::spawn(async move {
                if store.deliver(&hook, body.clone(), &did, &ev).await {
                    return;
                }
                store.persist_pending(&hook, did, ev, body);
            });
        }
    }

    /// Persist an undelivered webhook so it can be retried after a restart.
    /// Each delivery is a single JSON file under the pending_dir.
    fn persist_pending(&self, hook: &Webhook, db_id: String, event: String, body: Vec<u8>) {
        let pending = PendingDelivery {
            id: Uuid::new_v4().to_string(),
            db_id,
            event,
            hook: hook.clone(),
            attempts: hook.burst_attempts(),
            payload: body,
            created_at: clock_now_rfc3339(),
        };
        let path = format!("{}/{}.json", self.pending_dir, pending.id);
        if let Ok(raw) = serde_json::to_string_pretty(&pending)
            && let Err(e) = std::fs::write(&path, raw)
        {
            tracing::error!("Failed to queue pending webhook delivery: {}", e);
            return;
        }
        tracing::warn!(
            webhook = %pending.hook.id,
            url = %pending.hook.url,
            "Webhook delivery failed after all attempts; queued for later retry"
        );
    }

    /// Retry all queued deliveries once and remove each file on success.
    /// Public so a background loop can call it periodically.
    pub async fn retry_pending_once(&self) {
        let (max_attempts, ttl_secs) = pending_limits();
        let mut pending = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.pending_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = std::fs::read_to_string(&p)
                    && let Ok(d) = serde_json::from_str::<PendingDelivery>(&raw)
                {
                    pending.push((p, d));
                }
            }
        }
        for (path, mut d) in pending {
            if should_drop_pending(&d, max_attempts, ttl_secs) {
                tracing::warn!(
                    webhook = %d.hook.id,
                    url = %d.hook.url,
                    attempts = d.attempts,
                    created = %d.created_at,
                    "Webhook delivery expired; dropping queued delivery"
                );
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let delivered = self
                .deliver_with_backoff(
                    &d.hook,
                    d.payload.clone(),
                    &d.db_id,
                    &d.event,
                    &d.hook.in_memory_delays(),
                )
                .await;
            if delivered {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            d.attempts += 1;
            if let Ok(raw) = serde_json::to_string_pretty(&d)
                && let Err(e) = std::fs::write(&path, raw)
            {
                tracing::error!("Failed to update pending delivery attempts: {}", e);
            }
        }
    }

    /// Start a background task that periodically retries queued deliveries.
    pub fn spawn_retry_loop(&self) {
        let store = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                store.retry_pending_once().await;
            }
        });
    }

    async fn deliver(&self, hook: &Webhook, body: Vec<u8>, db_id: &str, event: &str) -> bool {
        let delays = hook.in_memory_delays();
        self.deliver_with_backoff(hook, body, db_id, event, &delays)
            .await
    }

    async fn deliver_with_backoff(
        &self,
        hook: &Webhook,
        body: Vec<u8>,
        db_id: &str,
        event: &str,
        backoffs: &[Duration],
    ) -> bool {
        if self.try_deliver(hook, &body, db_id, event).await {
            return true;
        }
        for delay in backoffs {
            tokio::time::sleep(*delay).await;
            if self.try_deliver(hook, &body, db_id, event).await {
                return true;
            }
        }
        false
    }

    async fn try_deliver(&self, hook: &Webhook, body: &[u8], db_id: &str, event: &str) -> bool {
        let mut req = self
            .client
            .post(&hook.url)
            .header("Content-Type", "application/json")
            .header("X-Turso-Event", event)
            .header("X-Turso-Database", db_id);
        for (name, value) in &hook.headers {
            if let (Ok(n), Ok(v)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                req = req.header(n, v);
            }
        }
        if !hook.secret.is_empty() {
            let sig = sign(&hook.secret, body);
            req = req.header("X-Turso-Signature", format!("{SIGNATURE_PREFIX}{sig}"));
        }
        match req.body(body.to_vec()).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(webhook = %hook.id, url = %hook.url, "Webhook delivered");
                true
            }
            Ok(resp) => {
                tracing::warn!(
                    webhook = %hook.id,
                    url = %hook.url,
                    status = %resp.status(),
                    "Webhook delivery failed"
                );
                false
            }
            Err(e) => {
                tracing::warn!(webhook = %hook.id, url = %hook.url, error = %e, "Webhook delivery error");
                false
            }
        }
    }
}

pub fn validate_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid webhook URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        other => Err(format!("webhook URL scheme must be http(s), got '{other}'")),
    }
}

/// Retry bounds for queued deliveries, from env with sane defaults.
/// - `WEBHOOK_PENDING_MAX_ATTEMPTS`: drop a delivery after this many retry ticks.
/// - `WEBHOOK_PENDING_TTL_SECS`: drop a delivery older than this many seconds.
pub fn pending_limits() -> (u32, u64) {
    let attempts = std::env::var("WEBHOOK_PENDING_MAX_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(PENDING_MAX_ATTEMPTS_DEFAULT);
    let ttl = std::env::var("WEBHOOK_PENDING_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(PENDING_TTL_DEFAULT_SECS);
    (attempts, ttl)
}

/// Whether a queued delivery should be dropped instead of retried, based on its attempts
/// counter and age. Malformed timestamps are treated as fresh (never immediately dropped).
fn should_drop_pending(d: &PendingDelivery, max_attempts: u32, ttl_secs: u64) -> bool {
    if d.attempts >= max_attempts {
        return true;
    }
    let age = chrono::DateTime::parse_from_rfc3339(&d.created_at)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds())
        .unwrap_or(0);
    age > ttl_secs as i64
}

pub fn validate_events(events: &[String]) -> Result<(), String> {
    if events.is_empty() {
        return Err("events must not be empty".to_string());
    }
    for e in events {
        if !ALLOWED_EVENTS.contains(&e.as_str()) {
            return Err(format!("unsupported event '{e}' (allowed: write, *)"));
        }
    }
    Ok(())
}

pub fn validate_headers(headers: &HashMap<String, String>) -> Result<(), String> {
    for (name, value) in headers {
        reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid webhook header name '{name}'"))?;
        reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("invalid webhook header value for '{name}'"))?;
    }
    Ok(())
}

pub fn validate_retry(retry: Option<&RetryPolicy>) -> Result<(), String> {
    let Some(p) = retry else {
        return Ok(());
    };
    if p.max_attempts == 0 || p.max_attempts > 100 {
        return Err("retry.max_attempts must be between 1 and 100".to_string());
    }
    if p.backoff_ms.len() > 20 {
        return Err("retry.backoff_ms must have at most 20 entries".to_string());
    }
    const MAX_BACKOFF_MS: u64 = 600_000; // 10 minutes
    for (i, ms) in p.backoff_ms.iter().enumerate() {
        if *ms == 0 {
            return Err(format!("retry.backoff_ms[{}] must be greater than 0", i));
        }
        if *ms > MAX_BACKOFF_MS {
            return Err(format!(
                "retry.backoff_ms[{}] exceeds {}ms max",
                i, MAX_BACKOFF_MS
            ));
        }
    }
    Ok(())
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Best-effort classification of a write statement into an operation and target table.
/// Returns `(op, table)` where `op` is one of insert/update/delete/ddl/other (None when the
/// statement shape is not recognized, e.g. a CTE or transactions spanning multiple statements).
pub fn classify_write(sql: &str) -> (Option<&'static str>, Option<String>) {
    let s = strip_comments(sql);
    if s.is_empty() {
        return (None, None);
    }
    let lower = s.to_ascii_lowercase();
    let starts = |k: &str| -> bool {
        lower.len() >= k.len()
            && &lower[..k.len()] == k
            && lower[k.len()..]
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric() && c != '_')
                .unwrap_or(true)
    };
    let take_table_after = |kw: &str| -> Option<String> {
        after_keyword(s, &lower, kw).and_then(|rest| take_ident(rest).0)
    };

    if starts("insert") {
        return (Some("insert"), take_table_after("into"));
    }
    if starts("update") {
        let mut rest = &s["update".len()..];
        let (first_word, after_first) = take_ident(rest);
        if first_word
            .as_deref()
            .is_some_and(|w| w.eq_ignore_ascii_case("or"))
        {
            let (_, after_behavior) = take_ident(after_first);
            rest = after_behavior;
        }
        return (Some("update"), take_ident(rest).0);
    }
    if starts("delete") {
        return (Some("delete"), take_table_after("from"));
    }
    if starts("create") || starts("alter") || starts("drop") {
        let table = if starts("alter") {
            None
        } else {
            after_keyword(s, &lower, "table").and_then(skip_create_qualifiers)
        };
        return (Some("ddl"), table);
    }
    if starts("reindex")
        || starts("vacuum")
        || starts("analyze")
        || starts("attach")
        || starts("detach")
    {
        return (Some("other"), None);
    }
    (None, None)
}

#[inline]
fn strip_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            s = match rest.find('\n') {
                Some(e) => &rest[e + 1..],
                None => return "",
            };
        } else if let Some(rest) = s.strip_prefix("/*") {
            match rest.find("*/") {
                Some(e) => s = &rest[e + 2..],
                None => return "",
            }
        } else {
            return s;
        }
    }
}

/// Returns the remainder of `sql` immediately after the keyword `kw` (word-boundary matched
/// against `lower`), preserving original casing.
fn after_keyword<'a>(sql: &'a str, lower: &str, kw: &str) -> Option<&'a str> {
    let mut idx = 0;
    while let Some(rel) = lower[idx..].find(kw) {
        let start = idx + rel;
        let end = start + kw.len();
        let before_ok = start == 0
            || !lower[..start]
                .chars()
                .next_back()
                .unwrap()
                .is_alphanumeric();
        let after_ok = lower[end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        if before_ok && after_ok {
            return Some(&sql[end..]);
        }
        idx = end;
    }
    None
}

/// Reads the next identifier (optionally quoted / bracketed) and returns it with the rest.
fn take_ident(s: &str) -> (Option<String>, &str) {
    let s = s.trim_start();
    let Some(first) = s.chars().next() else {
        return (None, s);
    };
    match first {
        '`' => match s[1..].find('`') {
            Some(e) => (Some(s[1..1 + e].to_string()), &s[1 + e + 1..]),
            None => (Some(s[1..].to_string()), ""),
        },
        '[' => match s[1..].find(']') {
            Some(e) => (Some(s[1..1 + e].to_string()), &s[1 + e + 1..]),
            None => (Some(s[1..].to_string()), ""),
        },
        '"' => match s[1..].find('"') {
            Some(e) => (Some(s[1..1 + e].to_string()), &s[1 + e + 1..]),
            None => (Some(s[1..].to_string()), ""),
        },
        c if c.is_ascii_alphabetic() || c == '_' => {
            let end = s
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(s.len());
            (Some(s[..end].to_string()), &s[end..])
        }
        _ => (None, s),
    }
}

/// Skips `CREATE TABLE` qualifiers (TEMP/TEMPORARY, IF NOT EXISTS) and returns the table name.
fn skip_create_qualifiers(s: &str) -> Option<String> {
    let mut rest = s;
    loop {
        let (word, after) = take_ident(rest);
        let skip = word
            .as_deref()
            .map(|w| {
                matches!(
                    w.to_ascii_lowercase().as_str(),
                    "temp" | "temporary" | "if" | "not" | "exists"
                )
            })
            .unwrap_or(false);
        if skip {
            rest = after;
        } else {
            return word;
        }
    }
}

pub fn change_events(statements: &[String], frames: Option<&[StatementFrame]>) -> Value {
    Value::Array(
        statements
            .iter()
            .enumerate()
            .map(|(i, sql)| {
                let (op, table) = classify_write(sql);
                let mut ev = json!({ "sql": sql, "op": op, "table": table });
                match op {
                    Some("insert") => ev["values"] = capture_insert_values(sql),
                    Some("update") => ev["values"] = capture_update_details(sql),
                    Some("delete") => ev["values"] = capture_delete_details(sql),
                    _ => {}
                }
                if let Some(frame) = frames.and_then(|f| f.get(i)).filter(|f| f.has_rows()) {
                    ev["frame"] = json!(frame);
                }
                ev
            })
            .collect(),
    )
}

/// Best-effort capture of the VALUES clause of an INSERT statement as
/// `{ "columns": [...], "rows": [[...], ...] }`. Returns null when the statement is not
/// a plain `INSERT ... VALUES` (e.g. `INSERT ... SELECT` or an unparsable clause), so
/// change events remain honest about what they know.
pub fn capture_insert_values(sql: &str) -> Value {
    let s = strip_comments(sql);
    let lower = s.to_ascii_lowercase();
    let Some(rest) = after_keyword(s, &lower, "values") else {
        return Value::Null;
    };
    let rows = parse_value_rows(rest);
    if rows.is_empty() {
        return Value::Null;
    }
    let columns = insert_columns(s, &lower);
    json!({ "columns": columns, "rows": rows })
}

/// Best-effort capture of a DELETE statement's WHERE clause as `{ "where": "<raw>" }`.
/// Returns null when there is no parseable WHERE (e.g. a full-table `DELETE FROM t`), so
/// consumers can distinguish "matched a subset" from "may have swept the whole table".
pub fn capture_delete_details(sql: &str) -> Value {
    let s = strip_comments(sql);
    let lower = s.to_ascii_lowercase();
    let Some(rest) = after_keyword(s, &lower, "from") else {
        return Value::Null;
    };
    capture_where_suffix(rest)
}

/// Best-effort capture of an UPDATE statement's SET assignments plus optional WHERE as
/// `{ "set": { "<col>": "<value>", ... }, "where": "<raw>" }`. `where` is omitted when
/// absent; returns null when neither is parseable.
pub fn capture_update_details(sql: &str) -> Value {
    let s = strip_comments(sql);
    let lower = s.to_ascii_lowercase();
    let Some(after_set) = after_keyword(s, &lower, "set") else {
        return Value::Null;
    };
    let sec_lower = after_set.to_ascii_lowercase();
    let (assign_src, where_raw) = match keyword_pos(&sec_lower, "where") {
        Some(pos) => (
            &after_set[..pos],
            Some(after_set[pos + "where".len()..].trim().to_string()),
        ),
        None => (after_set, None),
    };
    let set = parse_set_assignments(assign_src);
    if set.is_empty() && where_raw.as_deref().is_none_or(str::is_empty) {
        return Value::Null;
    }
    let mut obj = serde_json::Map::new();
    if !set.is_empty() {
        obj.insert("set".to_string(), Value::Object(set));
    }
    if let Some(w) = where_raw.filter(|w| !w.is_empty()) {
        obj.insert("where".to_string(), json!(w));
    }
    Value::Object(obj)
}

/// WHERE-clause raw suffix after a `from`/`delete from` header. Returns null when there is
/// no non-empty WHERE.
fn capture_where_suffix(rest: &str) -> Value {
    let rest_lower = rest.to_ascii_lowercase();
    match keyword_pos(&rest_lower, "where") {
        Some(pos) => {
            let w = rest[pos + "where".len()..].trim();
            if w.is_empty() {
                Value::Null
            } else {
                json!({ "where": w })
            }
        }
        None => Value::Null,
    }
}

/// Raw WHERE-clause text for an update/delete statement, used by change-framing to
/// snapshot the matched rows before executing the write.
pub(crate) fn where_text_for(sql: &str, op: &str) -> Option<String> {
    let s = strip_comments(sql);
    let lower = s.to_ascii_lowercase();
    match op {
        "delete" => {
            let rest = after_keyword(s, &lower, "from")?;
            capture_where_suffix(rest)
                .as_object()
                .and_then(|o| o.get("where"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
        "update" => {
            let rest = after_keyword(s, &lower, "set")?;
            find_top_level_word(rest, "where")
                .map(|pos| rest[pos + "where".len()..].trim().to_string())
        }
        _ => None,
    }
}

/// First occurrence of the word `kw` at parenthesis depth 0 and outside quoted strings
/// (case-insensitive, word-boundary guarded). Unlike `keyword_pos`, this skips matches
/// inside string literals and subqueries/parentheses.
fn find_top_level_word(s: &str, kw: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        match c {
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '(' | '[' if !in_single && !in_double && !in_backtick => depth += 1,
            ')' | ']' if !in_single && !in_double && !in_backtick => depth -= 1,
            _ => {}
        }
        if c.is_ascii_alphabetic()
            && depth == 0
            && !in_single
            && !in_double
            && !in_backtick
            && s[i..]
                .get(..kw.len())
                .is_some_and(|w| w.eq_ignore_ascii_case(kw))
            && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_'))
            && (i + kw.len() >= n
                || !(chars[i + kw.len()].is_alphanumeric() || chars[i + kw.len()] == '_'))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse `col = expr, col2 = expr, ...` into an object map, splitting only on top-level
/// commas (respecting quotes and nested parentheses) and top-level `=`.
fn parse_set_assignments(s: &str) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for raw in split_top_level(s, ',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let Some(eq) = find_top_level(part, '=') else {
            continue;
        };
        let col = take_ident(&part[..eq]).0.unwrap_or_default();
        let val = normalize_value(&part[eq + 1..]);
        if !col.is_empty() {
            out.insert(col, json!(val));
        }
    }
    out
}

/// Index of the first un-nested (parenthesis/bracket depth 0, outside quotes) occurrence
/// of `target`, or None.
fn find_top_level(s: &str, target: char) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '(' | '[' if !in_single && !in_double && !in_backtick => depth += 1,
            ')' | ']' if !in_single && !in_double && !in_backtick => depth -= 1,
            c2 if c2 == target && depth == 0 && !in_single && !in_double && !in_backtick => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

/// Split on `sep` only at nesting depth 0, outside quoted strings.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        match c {
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '(' | '[' if !in_single && !in_double && !in_backtick => depth += 1,
            ')' | ']' if !in_single && !in_double && !in_backtick => depth -= 1,
            c2 if c2 == sep && depth == 0 && !in_single && !in_double && !in_backtick => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

/// Index of the first word-boundary match of `kw` within `lower` (which must be the
/// lowercase form of the string being searched), matching `after_keyword` boundary rules.
fn keyword_pos(lower: &str, kw: &str) -> Option<usize> {
    let mut idx = 0;
    while let Some(rel) = lower[idx..].find(kw) {
        let start = idx + rel;
        let end = start + kw.len();
        let before_ok = start == 0
            || !lower[..start]
                .chars()
                .next_back()
                .unwrap()
                .is_alphanumeric();
        let after_ok = lower[end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        if before_ok && after_ok {
            return Some(start);
        }
        idx = end;
    }
    None
}

/// Tokenizer for `(v1, v2), (v3, v4)` tuples following a VALUES keyword. Handles single
/// quoted strings (with '' escapes), double-quoted/bracketed identifiers, nested calls
/// (commas inside parentheses are kept) and skips whitespace/comma separators between rows.
fn parse_value_rows(s: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut rows = Vec::new();
    let mut i = 0usize;
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        if chars[i] == ',' {
            i += 1;
            continue;
        }
        if chars[i] != '(' {
            break;
        }
        let mut depth = 0usize;
        let mut in_single = false;
        let mut in_double = false;
        let mut in_backtick = false;
        let mut token = String::new();
        let mut cell = Vec::new();
        let mut closed = false;
        loop {
            if i >= n {
                break;
            }
            let c = chars[i];
            match c {
                '\'' if !in_double && !in_backtick => {
                    in_single = !in_single;
                    token.push(c);
                }
                '"' if !in_single && !in_backtick => {
                    in_double = !in_double;
                    token.push(c);
                }
                '`' if !in_single && !in_double => {
                    in_backtick = !in_backtick;
                    token.push(c);
                }
                '(' if !in_single && !in_double && !in_backtick => {
                    depth += 1;
                    if depth > 1 {
                        token.push(c);
                    }
                }
                ')' if !in_single && !in_double && !in_backtick => {
                    if depth == 1 {
                        cell.push(normalize_value(&token));
                        closed = true;
                        break;
                    }
                    depth -= 1;
                    token.push(c);
                }
                ',' if !in_single && !in_double && !in_backtick && depth == 1 => {
                    cell.push(normalize_value(&token));
                    token.clear();
                }
                _ => token.push(c),
            }
            i += 1;
        }
        if closed {
            rows.push(cell);
            i += 1; // step past the closing ')'
        }
    }
    rows
}

fn normalize_value(t: &str) -> String {
    let t = t.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        t[1..t.len() - 1].replace("''", "'")
    } else {
        t.to_string()
    }
}

fn insert_columns(s: &str, lower: &str) -> Option<Vec<String>> {
    let rest = after_keyword(s, lower, "into")?;
    let (_, after_table) = take_ident(rest);
    let after_table = after_table.trim_start();
    if !after_table.starts_with('(') {
        return None;
    }
    parse_ident_list(after_table)
}

fn parse_ident_list(s: &str) -> Option<Vec<String>> {
    let rest = s.trim_start().strip_prefix('(')?;
    let mut rest = rest;
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        if rest.starts_with(')') {
            break;
        }
        let (ident, after) = take_ident(rest);
        out.push(ident?);
        rest = after.trim_start();
        match rest.chars().next() {
            Some(',') => rest = &rest[1..],
            Some(')') => break,
            _ => return Some(out),
        }
    }
    Some(out)
}

pub fn build_payload(
    db_id: &str,
    db_name: &str,
    owner: &str,
    event: &str,
    statements: &[String],
    rows_affected: u64,
    frames: &[StatementFrame],
) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "schema_version": 1,
        "delivery_id": Uuid::new_v4().to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "database": { "id": db_id, "name": db_name },
        "owner": owner,
        "statements": statements,
        "changes": change_events(statements, Some(frames)),
        "rows_affected": rows_affected,
    })
}

fn parse_hooks_value(v: Value) -> Vec<Webhook> {
    match v {
        Value::Array(items) => items
            .iter()
            .filter_map(|h| serde_json::from_value::<Webhook>(h.clone()).ok())
            .collect(),
        Value::String(s) => serde_json::from_str::<Vec<Webhook>>(&s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_signature_matches_known_vector() {
        // RFC-style published vector: HMAC-SHA256(key="key", msg="The quick brown fox jumps over the lazy dog")
        assert_eq!(
            sign("key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn signature_prefix_format() {
        let sig = sign("k", b"payload");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn classify_inserts() {
        assert_eq!(
            classify_write("INSERT INTO users (name) VALUES ('a')"),
            (Some("insert"), Some("users".to_string()))
        );
        assert_eq!(
            classify_write("INSERT OR REPLACE INTO `order items` (id) VALUES (1)"),
            (Some("insert"), Some("order items".to_string()))
        );
        assert_eq!(
            classify_write("  -- header comment\n /* block */ INSERT INTO main.t AS t VALUES (2)"),
            (Some("insert"), Some("main.t".to_string()))
        );
    }

    #[test]
    fn classify_updates_deletes_and_ddl() {
        assert_eq!(
            classify_write("UPDATE tasks SET status='done' WHERE id=1"),
            (Some("update"), Some("tasks".to_string()))
        );
        assert_eq!(
            classify_write("UPDATE OR IGNORE cache SET v=1"),
            (Some("update"), Some("cache".to_string()))
        );
        assert_eq!(
            classify_write("DELETE FROM projects WHERE id = 2"),
            (Some("delete"), Some("projects".to_string()))
        );
        assert_eq!(
            classify_write("CREATE TABLE IF NOT EXISTS logs (id INTEGER)"),
            (Some("ddl"), Some("logs".to_string()))
        );
        assert_eq!(
            classify_write("DROP TABLE old_stuff"),
            (Some("ddl"), Some("old_stuff".to_string()))
        );
        assert_eq!(
            classify_write("ALTER TABLE t ADD COLUMN c"),
            (Some("ddl"), None)
        );
        assert_eq!(classify_write("VACUUM"), (Some("other"), None));
    }

    #[test]
    fn classify_unrecognized_and_false_positive_guards() {
        assert_eq!(
            classify_write("WITH c AS (SELECT 1) INSERT INTO t SELECT * FROM c"),
            (None, None)
        );
        assert_eq!(classify_write("insertx AS foo"), (None, None));
    }

    #[test]
    fn captures_insert_values_with_columns() {
        let v = capture_insert_values("INSERT INTO users (name, age) VALUES ('Alice', 30)");
        assert_eq!(v["columns"], serde_json::json!(["name", "age"]));
        assert_eq!(v["rows"], serde_json::json!([["Alice", "30"]]));
    }

    #[test]
    fn captures_multi_row_and_quoted_values() {
        let v = capture_insert_values("INSERT INTO t (a) VALUES (1), (2), ('x,y'), ('it''s')");
        assert_eq!(
            v["rows"],
            serde_json::json!([["1"], ["2"], ["x,y"], ["it's"]])
        );

        let nested = capture_insert_values("INSERT INTO t VALUES (upper('x'))");
        assert_eq!(nested["rows"], serde_json::json!([["upper('x')"]]));
        assert_eq!(nested["columns"], serde_json::Value::Null);
    }

    #[test]
    fn insert_without_values_is_null() {
        assert_eq!(
            capture_insert_values("INSERT INTO t SELECT * FROM other"),
            serde_json::Value::Null
        );
        assert_eq!(
            capture_insert_values("UPDATE users SET name = 'x'"),
            serde_json::Value::Null
        );
    }

    #[test]
    fn change_events_attach_values_per_op() {
        let events = change_events(
            &[
                "INSERT INTO logs (msg) VALUES ('hi')".into(),
                "UPDATE logs SET msg = 'bye' WHERE id = 1".into(),
                "DELETE FROM logs WHERE id = 2".into(),
            ],
            None,
        );
        assert_eq!(events[0]["op"], "insert");
        assert_eq!(events[0]["values"]["rows"], serde_json::json!([["hi"]]));
        assert_eq!(events[1]["op"], "update");
        assert_eq!(
            events[1]["values"],
            serde_json::json!({ "set": { "msg": "bye" }, "where": "id = 1" })
        );
        assert_eq!(events[2]["op"], "delete");
        assert_eq!(
            events[2]["values"],
            serde_json::json!({ "where": "id = 2" })
        );
    }

    #[test]
    fn captures_update_set_and_where() {
        let v = capture_update_details(
            "UPDATE tasks SET status = 'done', priority = 2 WHERE id = 1 AND project = 'x'",
        );
        assert_eq!(
            v,
            serde_json::json!({
                "set": { "status": "done", "priority": "2" },
                "where": "id = 1 AND project = 'x'"
            })
        );
    }

    #[test]
    fn captures_update_without_where_and_function_values() {
        let v = capture_update_details("UPDATE OR IGNORE cache SET a = upper('x'), b = f(1, 2)");
        assert_eq!(
            v,
            serde_json::json!({
                "set": { "a": "upper('x')", "b": "f(1, 2)" }
            })
        );
        assert!(v.get("where").is_none(), "no WHERE → key omitted");
    }

    #[test]
    fn captures_delete_where_or_null() {
        assert_eq!(
            capture_delete_details("DELETE FROM projects WHERE id = 2"),
            serde_json::json!({ "where": "id = 2" })
        );
        assert_eq!(
            capture_delete_details("DELETE FROM t"),
            serde_json::Value::Null,
            "table sweep has no WHERE to report"
        );
        assert_eq!(
            capture_update_details("INSERT INTO t (a) VALUES (1)"),
            serde_json::Value::Null
        );
    }

    #[test]
    fn event_matching() {
        let wildcard = Webhook {
            id: "1".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["*".into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        let write_only = Webhook {
            id: "2".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        let other = Webhook {
            id: "3".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["insert".into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        assert!(wildcard.matches(EVENT_WRITE));
        assert!(write_only.matches(EVENT_WRITE));
        assert!(!other.matches(EVENT_WRITE));
    }

    #[test]
    fn url_validation() {
        assert!(validate_url("https://hooks.example.com/cb").is_ok());
        assert!(validate_url("http://localhost:9000/hook").is_ok());
        assert!(validate_url("ftp://example.com/x").is_err());
        assert!(validate_url("not a url").is_err());
    }

    #[test]
    fn events_validation() {
        assert!(validate_events(&["write".into()]).is_ok());
        assert!(validate_events(&["*".into()]).is_ok());
        assert!(validate_events(&["insert".into(), "write".into()]).is_err());
        assert!(validate_events(&[]).is_err());
    }

    #[test]
    fn headers_validation() {
        let ok = HashMap::from([("X-Custom".to_string(), "yes".to_string())]);
        let bad_name = HashMap::from([("Bad Name".to_string(), "x".to_string())]);
        let bad_value = HashMap::from([("X-Custom".to_string(), "line\nfeed".to_string())]);
        assert!(validate_headers(&ok).is_ok());
        assert!(validate_headers(&bad_name).is_err());
        assert!(validate_headers(&bad_value).is_err());
    }

    #[test]
    fn retry_policy_validation_and_defaults() {
        let default = RetryPolicy::default();
        assert_eq!(default.max_attempts, 5);
        assert_eq!(default.backoff_ms, vec![1000, 2000, 4000, 8000]);
        assert!(validate_retry(Some(&default)).is_ok());
        assert!(validate_retry(None).is_ok());

        let bad_max = RetryPolicy {
            max_attempts: 0,
            backoff_ms: vec![1],
        };
        assert!(validate_retry(Some(&bad_max)).is_err());
        let too_many = RetryPolicy {
            max_attempts: 5,
            backoff_ms: vec![1; 21],
        };
        assert!(validate_retry(Some(&too_many)).is_err());
        let zero_delay = RetryPolicy {
            max_attempts: 5,
            backoff_ms: vec![0],
        };
        assert!(validate_retry(Some(&zero_delay)).is_err());
        let huge_delay = RetryPolicy {
            max_attempts: 5,
            backoff_ms: vec![999_999],
        };
        assert!(validate_retry(Some(&huge_delay)).is_err());
    }

    #[test]
    fn hook_burst_respects_policy() {
        let hook = Webhook {
            id: "1".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            retry: Some(RetryPolicy {
                max_attempts: 3,
                backoff_ms: vec![50, 100, 200],
            }),
            created_at: String::new(),
        };
        let delays = hook.in_memory_delays();
        assert_eq!(delays.len(), 2, "capped at max_attempts - 1");
        assert_eq!(delays[0], Duration::from_millis(50));
        assert_eq!(delays[1], Duration::from_millis(100));
        assert_eq!(hook.burst_attempts(), 3);

        let no_policy = Webhook {
            retry: None,
            ..hook.clone()
        };
        assert_eq!(no_policy.burst_attempts(), 5, "default = 1 + 4 backoffs");
    }

    #[test]
    fn store_roundtrip_persists() {
        let dir = std::env::temp_dir().join(format!("turso-wb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("webhooks.json").to_string_lossy().into_owned();

        let store = WebhookStore::new(&path, None);
        let hook = store
            .add(
                "db-1",
                "https://example.com/h",
                Some("s3cret".into()),
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(store.list("db-1").len(), 1);

        let reloaded = WebhookStore::new(&path, None);
        assert_eq!(reloaded.list("db-1").len(), 1);
        assert_eq!(reloaded.list("db-1")[0].id, hook.id);
        assert_eq!(reloaded.list("db-1")[0].secret, "s3cret");

        assert!(reloaded.remove("db-1", &hook.id));
        assert!(!reloaded.remove("db-1", &hook.id));
        assert!(reloaded.list("db-1").is_empty());

        reloaded.remove_all("db-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_shape() {
        let p = build_payload(
            "db-1",
            "mydb",
            "alice",
            EVENT_WRITE,
            &["INSERT INTO t VALUES (3)".into()],
            2,
            &[],
        );
        assert_eq!(p["event"], "write");
        assert_eq!(p["database"]["id"], "db-1");
        assert_eq!(p["database"]["name"], "mydb");
        assert_eq!(p["owner"], "alice");
        assert_eq!(p["statements"][0], "INSERT INTO t VALUES (3)");
        assert_eq!(p["changes"][0]["op"], "insert");
        assert_eq!(p["changes"][0]["table"], "t");
        assert_eq!(p["rows_affected"], 2);
        assert_eq!(p["schema_version"], 1);
        assert!(p["delivery_id"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn payload_includes_frames_when_present() {
        let frame = StatementFrame {
            op: Some("update".to_string()),
            table: Some("t".to_string()),
            before: Some(RowFrame {
                columns: vec!["id".into(), "name".into()],
                rows: vec![vec!["1".into(), "old".into()]],
            }),
            after: Some(RowFrame {
                columns: vec!["id".into(), "name".into()],
                rows: vec![vec!["1".into(), "new".into()]],
            }),
        };
        let empty = StatementFrame::default();
        let p = build_payload(
            "db-1",
            "mydb",
            "alice",
            EVENT_WRITE,
            &["UPDATE t SET name='new' WHERE id=1".into()],
            1,
            &[frame, empty],
        );
        let ch = &p["changes"][0];
        assert_eq!(ch["op"], "update");
        assert_eq!(ch["frame"]["before"]["rows"][0][0], "1");
        assert_eq!(ch["frame"]["before"]["rows"][0][1], "old");
        assert_eq!(ch["frame"]["after"]["rows"][0][1], "new");
        assert_eq!(p["changes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delivers_signed_payload_to_http_target() {
        use axum::{
            Router,
            body::Bytes,
            extract::State,
            http::{HeaderMap, StatusCode},
            routing::post,
        };
        use std::time::Duration;
        use tokio::sync::mpsc;

        async fn receiver_handler(
            headers: HeaderMap,
            State(sender): State<mpsc::UnboundedSender<(HeaderMap, Bytes)>>,
            body: Bytes,
        ) -> StatusCode {
            let _ = sender.send((headers, body));
            StatusCode::OK
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/hook", post(receiver_handler))
            .with_state(tx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = std::env::temp_dir().join(format!("turso-wb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = WebhookStore::new(dir.join("wb.json").to_str().unwrap(), None);
        let mut extra = HashMap::new();
        extra.insert("X-Custom".to_string(), "hello".to_string());
        store
            .add(
                "db-x",
                &format!("http://{addr}/hook"),
                Some("s3cret".into()),
                None,
                Some(extra),
                None,
            )
            .unwrap();
        store.dispatch(
            "db-x",
            "myname",
            "owner",
            EVENT_WRITE,
            vec!["INSERT INTO t VALUES (1)".into()],
            1,
            Vec::new(),
        );

        let (headers, body) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("webhook delivery timed out")
            .expect("delivery channel closed");

        let sig = headers
            .get("x-turso-signature")
            .expect("signature header")
            .to_str()
            .unwrap();
        assert_eq!(sig, format!("sha256={}", sign("s3cret", &body)));
        assert_eq!(
            headers.get("x-turso-event").unwrap().to_str().unwrap(),
            EVENT_WRITE
        );
        assert_eq!(
            headers.get("x-turso-database").unwrap().to_str().unwrap(),
            "db-x"
        );
        assert_eq!(headers.get("x-custom").unwrap().to_str().unwrap(), "hello");
        assert_eq!(
            headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .to_lowercase(),
            "application/json"
        );

        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["event"], "write");
        assert_eq!(v["database"]["id"], "db-x");
        assert_eq!(v["database"]["name"], "myname");
        assert_eq!(v["owner"], "owner");
        assert_eq!(v["rows_affected"], 1);
        assert_eq!(v["statements"][0], "INSERT INTO t VALUES (1)");
        assert_eq!(v["changes"][0]["op"], "insert");
        assert_eq!(v["changes"][0]["table"], "t");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn retries_until_success() {
        use axum::{Router, extract::State, http::StatusCode, routing::post};
        use std::sync::atomic::{AtomicU8, Ordering};

        async fn flaky_handler(State(count): State<Arc<AtomicU8>>) -> StatusCode {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::OK
            }
        }

        let hits = Arc::new(AtomicU8::new(0));
        let app = Router::new()
            .route("/flaky", post(flaky_handler))
            .with_state(hits.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = std::env::temp_dir().join(format!("turso-wb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = WebhookStore::new(dir.join("wb.json").to_str().unwrap(), None);
        let hook = Webhook {
            id: "r1".into(),
            url: format!("http://{addr}/flaky"),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        let backoffs = [Duration::from_millis(10), Duration::from_millis(10)];

        let delivered = store
            .deliver_with_backoff(&hook, b"{}".to_vec(), "db-r", EVENT_WRITE, &backoffs)
            .await;
        assert!(delivered, "delivery should eventually succeed");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            3,
            "expected 2 failures then 1 success"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn gives_up_after_backoffs_exhausted() {
        use axum::{Router, extract::State, http::StatusCode, routing::post};
        use std::sync::atomic::{AtomicU8, Ordering};

        async fn always_fail_handler(State(count): State<Arc<AtomicU8>>) -> StatusCode {
            count.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR
        }

        let hits = Arc::new(AtomicU8::new(0));
        let app = Router::new()
            .route("/bady", post(always_fail_handler))
            .with_state(hits.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = std::env::temp_dir().join(format!("turso-wb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = WebhookStore::new(dir.join("wb.json").to_str().unwrap(), None);
        let hook = Webhook {
            id: "r2".into(),
            url: format!("http://{addr}/bady"),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        let backoffs = [Duration::from_millis(5), Duration::from_millis(5)];

        let delivered = store
            .deliver_with_backoff(&hook, b"{}".to_vec(), "db-r", EVENT_WRITE, &backoffs)
            .await;
        assert!(!delivered);
        assert_eq!(hits.load(Ordering::SeqCst), 3, "1 + backoff count attempts");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failed_delivery_is_queued_and_retried_later() {
        use axum::{Router, extract::State, http::StatusCode, routing::post};
        use std::sync::atomic::{AtomicU8, Ordering};

        // A handler that fails the first call then succeeds.
        async fn flaky_handler(State(count): State<Arc<AtomicU8>>) -> StatusCode {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n < 1 {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            }
        }

        let hits = Arc::new(AtomicU8::new(0));
        let app = Router::new()
            .route("/q", post(flaky_handler))
            .with_state(hits.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = std::env::temp_dir().join(format!("turso-q-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate a failed delivery being queued for later (e.g. receiver was down).
        let store1 = WebhookStore::new(dir.join("w1.json").to_str().unwrap(), None);
        let hook1 = Webhook {
            id: "q1".into(),
            url: format!("http://{addr}/q"),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            retry: None,
            created_at: String::new(),
        };
        store1.persist_pending(&hook1, "db-q".into(), EVENT_WRITE.into(), b"{}".to_vec());
        let after_persist = std::fs::read_dir(&store1.pending_dir).unwrap().count();
        assert_eq!(
            after_persist, 1,
            "expected one queued delivery file after persist"
        );

        // On a "restart", a fresh store finds the file, attempts delivery, and on the
        // backoff the target (which failed on call 1 but succeeds on call 2) accepts, so
        // the pending file is removed.
        let store2 = WebhookStore::new(dir.join("w1.json").to_str().unwrap(), None);
        store2.retry_pending_once().await;

        let remaining = std::fs::read_dir(&store2.pending_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(
            remaining, 0,
            "pending file should be removed after successful retry"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_delivery_roundtrip_serializes() {
        let d = PendingDelivery {
            id: "p1".into(),
            db_id: "db-1".into(),
            event: EVENT_WRITE.into(),
            hook: Webhook {
                id: "h1".into(),
                url: "https://x.test".into(),
                secret: "s".into(),
                events: vec![EVENT_WRITE.into()],
                headers: HashMap::new(),
                retry: None,
                created_at: "now".into(),
            },
            attempts: 5,
            payload: b"{}".to_vec(),
            created_at: clock_now_rfc3339(),
        };
        let raw = serde_json::to_string(&d).unwrap();
        let back: PendingDelivery = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.id, "p1");
        assert_eq!(back.hook.secret, "s");
        assert_eq!(back.attempts, 5);
    }

    #[test]
    fn legacy_pending_file_without_created_at_parses() {
        let raw = r#"{"id":"p1","db_id":"db-1","event":"write",
                     "hook":{"id":"h1","url":"https://x.test","secret":"s",
                             "events":["write"],"headers":{},"created_at":"now"},
                     "attempts":5,"payload":[123,125]}"#;
        let parsed: PendingDelivery = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.id, "p1");
        assert_eq!(parsed.attempts, 5);
        assert!(
            !parsed.created_at.is_empty(),
            "created_at must default for legacy files"
        );
    }

    #[test]
    fn pending_delivery_drop_rules() {
        let base = PendingDelivery {
            id: "p1".into(),
            db_id: "db-1".into(),
            event: EVENT_WRITE.into(),
            hook: Webhook {
                id: "h1".into(),
                url: "https://x.test".into(),
                secret: "s".into(),
                events: vec![EVENT_WRITE.into()],
                headers: HashMap::new(),
                retry: None,
                created_at: "now".into(),
            },
            attempts: 5,
            payload: b"{}".to_vec(),
            created_at: clock_now_rfc3339(),
        };

        // Recent delivery within limits → keep.
        assert!(!should_drop_pending(&base, 10_000, 604_800));

        // Attempts exhausted → drop.
        let exhausted = PendingDelivery {
            attempts: 500,
            ..base.clone()
        };
        assert!(should_drop_pending(&exhausted, 500, 604_800));

        // Old delivery past the TTL → drop.
        let old = PendingDelivery {
            created_at: "2020-01-01T00:00:00Z".into(),
            ..base.clone()
        };
        assert!(should_drop_pending(&old, 10_000, 3600));

        // Malformed timestamp treated as fresh → keep.
        let malformed = PendingDelivery {
            created_at: "not-a-timestamp".into(),
            ..base.clone()
        };
        assert!(!should_drop_pending(&malformed, 10_000, 1));
    }

    #[test]
    fn pending_limits_env_overrides() {
        let (attempts, ttl) = pending_limits();
        assert!(attempts > 0 && ttl > 0);
        assert_eq!(PENDING_MAX_ATTEMPTS_DEFAULT, 10_080);
        assert_eq!(PENDING_TTL_DEFAULT_SECS, 604_800);
    }
}
