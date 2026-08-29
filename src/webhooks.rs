use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub const EVENT_WRITE: &str = "write";
const ALLOWED_EVENTS: [&str; 2] = [EVENT_WRITE, "*"];
const SIGNATURE_PREFIX: &str = "sha256=";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default = "default_events")]
    pub events: Vec<String>,
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

#[derive(Clone)]
pub struct WebhookStore {
    path: String,
    hooks: Arc<DashMap<String, Vec<Webhook>>>,
    client: reqwest::Client,
}

impl WebhookStore {
    pub fn new(path: &str) -> Self {
        let store = Self {
            path: path.to_string(),
            hooks: Arc::new(DashMap::new()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        };
        store.load();
        store
    }

    fn load(&self) {
        if !Path::new(&self.path).exists() {
            return;
        }
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

    fn save(&self) {
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
    ) -> Result<Webhook, String> {
        validate_url(url)?;
        let events = events.unwrap_or_else(default_events);
        validate_events(&events)?;
        let hook = Webhook {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            secret: secret.unwrap_or_default(),
            events,
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
        }
        removed
    }

    pub fn remove_all(&self, db_id: &str) {
        if self.hooks.remove(db_id).is_some() {
            self.save();
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
            tokio::spawn(async move { store.deliver(hook, body, &did, &ev).await });
        }
    }

    async fn deliver(&self, hook: Webhook, body: Vec<u8>, db_id: &str, event: &str) {
        let mut req = self
            .client
            .post(&hook.url)
            .header("Content-Type", "application/json")
            .header("X-Turso-Event", event)
            .header("X-Turso-Database", db_id);
        if !hook.secret.is_empty() {
            let sig = sign(&hook.secret, &body);
            req = req.header("X-Turso-Signature", format!("{SIGNATURE_PREFIX}{sig}"));
        }
        match req.body(body).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::debug!(
                        webhook = %hook.id,
                        url = %hook.url,
                        "Webhook delivered"
                    );
                } else {
                    tracing::warn!(
                        webhook = %hook.id,
                        url = %hook.url,
                        status = %resp.status(),
                        "Webhook delivery failed"
                    );
                }
            }
            Err(e) => tracing::warn!(
                webhook = %hook.id,
                url = %hook.url,
                error = %e,
                "Webhook delivery error"
            ),
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

pub fn sign(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
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
        "rows_affected": rows_affected,
    })
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
    fn event_matching() {
        let wildcard = Webhook {
            id: "1".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["*".into()],
            created_at: String::new(),
        };
        let write_only = Webhook {
            id: "2".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec![EVENT_WRITE.into()],
            created_at: String::new(),
        };
        let other = Webhook {
            id: "3".into(),
            url: "https://x.test".into(),
            secret: String::new(),
            events: vec!["insert".into()],
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
    fn store_roundtrip_persists() {
        let dir = std::env::temp_dir().join(format!("turso-wb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("webhooks.json").to_string_lossy().into_owned();

        let store = WebhookStore::new(&path);
        let hook = store
            .add("db-1", "https://example.com/h", Some("s3cret".into()), None)
            .unwrap();
        assert_eq!(store.list("db-1").len(), 1);

        let reloaded = WebhookStore::new(&path);
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
            &["INSERT...".into()],
            2,
        );
        assert_eq!(p["event"], "write");
        assert_eq!(p["database"]["id"], "db-1");
        assert_eq!(p["database"]["name"], "mydb");
        assert_eq!(p["owner"], "alice");
        assert_eq!(p["statements"][0], "INSERT...");
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
        let store = WebhookStore::new(dir.join("wb.json").to_str().unwrap());
        store
            .add(
                "db-x",
                &format!("http://{addr}/hook"),
                Some("s3cret".into()),
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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
