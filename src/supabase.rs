use reqwest::Client;
use serde_json::Value;
use std::env;

const BUCKET: &str = "turso-dbs";

#[derive(Clone)]
pub struct Supabase {
    client: Client,
    base_url: String,
    service_key: String,
}

impl Supabase {
    pub fn from_env() -> Option<Self> {
        let url = env::var("SUPABASE_URL").ok()?.trim_end_matches('/').to_string();
        let key = env::var("SUPABASE_SERVICE_KEY").ok()?;
        if url.is_empty() || key.is_empty() {
            return None;
        }
        Some(Self {
            client: Client::builder().build().ok()?,
            base_url: url,
            service_key: key,
        })
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("apikey", self.service_key.parse().unwrap());
        h.insert(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.service_key).parse().unwrap());
        h
    }

    fn headers_json(&self) -> reqwest::header::HeaderMap {
        let mut h = self.headers();
        h.insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
        h
    }

    pub async fn rows(&self, table: &str, filter: &str) -> Result<Vec<Value>, String> {
        let url = format!("{}/rest/v1/{}?select=*{}", self.base_url, table, filter);
        let resp = self.client.get(&url).headers(self.headers()).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("supabase select {}: {} {}", table, status, body));
        }
        serde_json::from_str(&body).map_err(|e| format!("supabase parse: {}", e))
    }

    pub async fn insert(&self, table: &str, row: Value) -> Result<Value, String> {
        let url = format!("{}/rest/v1/{}", self.base_url, table);
        let resp = self.client.post(&url)
            .headers(self.headers_json())
            .header("Prefer", "return=representation")
            .json(&row)
            .send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("supabase insert {}: {} {}", table, status, body));
        }
        serde_json::from_str::<Vec<Value>>(&body).map_err(|e| e.to_string())?.into_iter().next().ok_or_else(|| "no row returned".into())
    }

    pub async fn upsert(&self, table: &str, rows: Value) -> Result<(), String> {
        let url = format!("{}/rest/v1/{}", self.base_url, table);
        let resp = self.client.post(&url)
            .headers(self.headers_json())
            .header("Prefer", "resolution=merge-duplicates")
            .json(&rows)
            .send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("supabase upsert {}: {} {}", table, status, body));
        }
        Ok(())
    }

    pub async fn update(&self, table: &str, filter: &str, patch: Value) -> Result<(), String> {
        let url = format!("{}/rest/v1/{}?{}", self.base_url, table, filter);
        let resp = self.client.patch(&url)
            .headers(self.headers_json())
            .json(&patch)
            .send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("supabase update {}: {} {}", table, status, body));
        }
        Ok(())
    }

    pub async fn delete(&self, table: &str, filter: &str) -> Result<(), String> {
        let url = format!("{}/rest/v1/{}?{}", self.base_url, table, filter);
        let resp = self.client.delete(&url)
            .headers(self.headers())
            .send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("supabase delete {}: {} {}", table, status, body));
        }
        Ok(())
    }

    pub async fn upload_db(&self, owner: &str, id: &str, bytes: Vec<u8>) -> Result<(), String> {
        let path = format!("{}/{}.db", owner, id);
        let url = format!("{}/storage/v1/object/{}/{}", self.base_url, BUCKET, path);
        let resp = self.client.post(&url)
            .headers(self.headers())
            .header("Content-Type", "application/octet-stream")
            .header("x-upsert", "true")
            .body(bytes)
            .send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("supabase upload {}: {} {}", path, status, body));
        }
        Ok(())
    }

    pub async fn download_db(&self, owner: &str, id: &str) -> Result<Vec<u8>, String> {
        let path = format!("{}/{}.db", owner, id);
        let url = format!("{}/storage/v1/object/{}/{}", self.base_url, BUCKET, path);
        let resp = self.client.get(&url).headers(self.headers()).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("supabase download {}: {}", path, status));
        }
        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
    }

    pub async fn delete_db(&self, owner: &str, id: &str) -> Result<(), String> {
        let path = format!("{}/{}.db", owner, id);
        let url = format!("{}/storage/v1/object/{}/{}", self.base_url, BUCKET, path);
        let resp = self.client.delete(&url).headers(self.headers()).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 400 && status.as_u16() != 404 {
            return Err(format!("supabase delete object {}: {}", path, status));
        }
        Ok(())
    }

    pub fn url(&self) -> &str {
        &self.base_url
    }
}
