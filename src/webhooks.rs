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
    pub created_at: String,
}

fn default_events() -> Vec<String> {
    vec![EVENT_WRITE.to_string()]
}

impl Webhook {
    pub fn matches(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == "*" || e == event)
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
    ) -> Result<Webhook, String> {
        validate_url(url)?;
        let events = events.unwrap_or_else(default_events);
        validate_events(&events)?;
        let headers = headers.unwrap_or_default();
        validate_headers(&headers)?;
        let hook = Webhook {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            secret: secret.unwrap_or_default(),
            events,
            headers,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.hooks
            .entry(db_id.to_string())
            .or_default()
            .push(hook.clone());
        self.save();
        Ok(hook)
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

    pub fn dispatch(
        &self,
        db_id: &str,
        db_name: &str,
        owner: &str,
        event: &str,
        statements: Vec<String>,
        rows_affected: u64,
    ) {
        let hooks = self.list(db_id);
        let receivers: Vec<Webhook> = hooks.into_iter().filter(|h| h.matches(event)).collect();
        let body = build_payload(db_id, db_name, owner, event, &statements, rows_affected);
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
            attempts: DEFAULT_BACKOFF.len() as u32 + 1,
            payload: body,
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
        for (path, d) in pending {
            let delivered = self
                .deliver_with_backoff(&d.hook, d.payload, &d.db_id, &d.event, &DEFAULT_BACKOFF)
                .await;
            if delivered {
                let _ = std::fs::remove_file(&path);
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
        self.deliver_with_backoff(hook, body, db_id, event, &DEFAULT_BACKOFF)
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

pub fn change_events(statements: &[String]) -> Value {
    Value::Array(
        statements
            .iter()
            .map(|sql| {
                let (op, table) = classify_write(sql);
                json!({ "sql": sql, "op": op, "table": table })
            })
            .collect(),
    )
}

pub fn build_payload(
    db_id: &str,
    db_name: &str,
    owner: &str,
    event: &str,
    statements: &[String],
    rows_affected: u64,
) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "schema_version": 1,
        "delivery_id": Uuid::new_v4().to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "database": { "id": db_id, "name": db_name },
        "owner": owner,
        "statements": statements,
        "changes": change_events(statements),
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
    fn event_matching() {
        let wildcard = Webhook {
            id: "1".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["*".into()],
            headers: HashMap::new(),
            created_at: String::new(),
        };
        let write_only = Webhook {
            id: "2".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            headers: HashMap::new(),
            created_at: String::new(),
        };
        let other = Webhook {
            id: "3".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["insert".into()],
            headers: HashMap::new(),
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
            )
            .unwrap();
        store.dispatch(
            "db-x",
            "myname",
            "owner",
            EVENT_WRITE,
            vec!["INSERT INTO t VALUES (1)".into()],
            1,
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
                created_at: "now".into(),
            },
            attempts: 5,
            payload: b"{}".to_vec(),
        };
        let raw = serde_json::to_string(&d).unwrap();
        let back: PendingDelivery = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.id, "p1");
        assert_eq!(back.hook.secret, "s");
        assert_eq!(back.attempts, 5);
    }
}
