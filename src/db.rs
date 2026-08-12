use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use turso::Builder;
use uuid::Uuid;

use crate::supabase::Supabase;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct DatabaseEntry {
    pub owner: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct DatabaseManager {
    data_dir: String,
    manifest_path: String,
    databases: Arc<DashMap<String, (turso::Database, DatabaseEntry)>>,
    supabase: Option<Supabase>,
}

impl DatabaseManager {
    pub async fn new(data_dir: &str, supabase: Option<Supabase>) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let manager = Self {
            data_dir: data_dir.to_string(),
            manifest_path: format!("{}/databases.json", data_dir),
            databases: Arc::new(DashMap::new()),
            supabase,
        };
        if manager.supabase.is_some() {
            manager.load_from_supabase().await?;
        } else {
            manager.load_manifest().await?;
        }
        Ok(manager)
    }

    async fn load_manifest(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut recovered = false;
        if Path::new(&self.manifest_path).exists() {
            let raw = std::fs::read_to_string(&self.manifest_path)?;
            if let Ok(entries) = serde_json::from_str::<Vec<ManifestEntry>>(&raw) {
                for entry in entries {
                    let path = format!("{}/{}.db", self.data_dir, entry.id);
                    if !Path::new(&path).exists() { continue; }
                    match Builder::new_local(&path).build().await {
                        Ok(db) => {
                            self.databases.insert(entry.id.clone(), (db, DatabaseEntry { owner: entry.owner.clone(), name: entry.name.clone(), created_at: entry.created_at.clone() }));
                            tracing::info!("Reloaded database: {} ({})", entry.name, entry.id);
                        }
                        Err(e) => tracing::warn!("Failed to reload database {}: {}", entry.id, e),
                    }
                }
            }
        }

        let mut known: Vec<String> = self.databases.iter().map(|entry| entry.key().clone()).collect();
        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(|n| n.to_string()) else { continue; };
            if !file_name.ends_with(".db") || file_name == "auth.db" || file_name == "users.json" { continue; }
            let id = file_name.trim_end_matches(".db").to_string();
            if known.contains(&id) { continue; }
            let db_path = path.to_string_lossy().into_owned();
            match Builder::new_local(&db_path).build().await {
                Ok(db) => {
                    self.databases.insert(id.clone(), (db, DatabaseEntry { owner: "admin".to_string(), name: format!("recovered-{}", &id), created_at: chrono::Utc::now().to_rfc3339() }));
                    tracing::warn!("Recovered orphan database file {} as owner admin", id);
                    recovered = true;
                }
                Err(e) => tracing::warn!("Failed to recover database file {}: {}", file_name, e),
            }
        }
        if recovered { self.save_manifest()?; }
        Ok(())
    }

    async fn load_from_supabase(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sb = self.supabase.as_ref().unwrap();
        let rows = sb.rows("turso_databases", "").await?;
        let mut ids: Vec<String> = Vec::new();
        for row in rows {
            let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let owner = row.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let created_at = row.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() { continue; }
            ids.push(id.clone());
            let path = format!("{}/{}.db", self.data_dir, id);
            if !Path::new(&path).exists() {
                match sb.download_db(&owner, &id).await {
                    Ok(bytes) => {
                        tokio::fs::write(&path, bytes).await?;
                        tracing::info!("Restored database {} ({}) from storage", name, id);
                    }
                    Err(_) => {
                        tracing::warn!("No stored backup for {}, creating empty", name);
                        Builder::new_local(&path).build().await?;
                    }
                }
            }
            match Builder::new_local(&path).build().await {
                Ok(db) => {
                    self.databases.insert(id.clone(), (db, DatabaseEntry { owner, name, created_at }));
                    tracing::info!("Loaded database from registry: {}", id);
                }
                Err(e) => tracing::warn!("Failed to open database {}: {}", id, e),
            }
        }

        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(|n| n.to_string()) else { continue; };
            if !file_name.ends_with(".db") || file_name == "auth.db" { continue; }
            let id = file_name.trim_end_matches(".db").to_string();
            if !ids.contains(&id) {
                let _ = std::fs::remove_file(&path);
                tracing::warn!("Removed orphan database file not in registry: {}", id);
            }
        }
        Ok(())
    }

    fn save_manifest(&self) -> Result<(), Box<dyn std::error::Error>> {
        let entries: Vec<ManifestEntry> = self.databases.iter().map(|entry| {
            let id = entry.key().clone();
            let meta = entry.value().1.clone();
            ManifestEntry { id, owner: meta.owner, name: meta.name, created_at: meta.created_at }
        }).collect();
        let raw = serde_json::to_string_pretty(&entries)?;
        std::fs::write(&self.manifest_path, raw)?;
        Ok(())
    }

    pub async fn create_database(&self, name: &str, owner: &str) -> Result<(String, DatabaseEntry), Box<dyn std::error::Error>> {
        let id = Uuid::new_v4().to_string();
        let path = format!("{}/{}.db", self.data_dir, id);
        let db = Builder::new_local(&path).build().await?;
        let entry = DatabaseEntry {
            owner: owner.to_string(),
            name: name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.databases.insert(id.clone(), (db, entry.clone()));
        if let Some(sb) = &self.supabase {
            let _ = sb.insert("turso_databases", serde_json::json!({
                "id": id,
                "name": entry.name,
                "owner": entry.owner,
                "created_at": entry.created_at,
            })).await;
            let bytes = tokio::fs::read(&path).await?;
            let _ = sb.upload_db(&entry.owner, &id, bytes).await;
        } else {
            self.save_manifest()?;
        }
        tracing::info!("Created database: {} (owner: {}) at {}", name, owner, path);
        Ok((id, entry))
    }

    pub async fn get_database(&self, id: &str) -> Result<(turso::Database, DatabaseEntry), Box<dyn std::error::Error>> {
        self.databases
            .get(id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("Database {} not found", id).into())
    }

    async fn persist_db(&self, db_id: &str, owner: &str) -> Result<(), Box<dyn std::error::Error>> {
        let Some(sb) = &self.supabase else { return Ok(()) };
        let path = format!("{}/{}.db", self.data_dir, db_id);
        if let Some((db, _)) = self.databases.get(db_id) {
            if let Ok(conn) = db.connect() {
                let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE);", ()).await;
            }
        }
        let bytes = tokio::fs::read(&path).await?;
        sb.upload_db(owner, db_id, bytes).await?;
        Ok(())
    }

    pub async fn execute(&self, db_id: &str, sql: &str) -> Result<String, Box<dyn std::error::Error>> {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let result = conn.execute(sql, ()).await?;
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok(format!("{} rows affected", result))
    }

    pub async fn query(&self, db_id: &str, sql: &str) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let mut rows = conn.query(sql, ()).await?;
        let mut results = Vec::new();

        while let Some(row) = rows.next().await? {
            let mut row_data = Vec::new();
            for i in 0..row.column_count() {
                let value = row.get_value(i)
                    .map(|v| {
                        match v {
                            turso::Value::Null => "NULL".to_string(),
                            turso::Value::Integer(n) => n.to_string(),
                            turso::Value::Real(f) => f.to_string(),
                            turso::Value::Text(s) => s,
                            turso::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
                        }
                    })
                    .unwrap_or_else(|_| "?".to_string());
                row_data.push(value);
            }
            results.push(row_data);
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok(results)
    }

    pub async fn delete_database(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let owner = self.databases.get(id).map(|e| e.value().1.owner.clone());
        self.databases.remove(id);
        let path = format!("{}/{}.db", self.data_dir, id);
        if Path::new(&path).exists() {
            std::fs::remove_file(path)?;
        }
        if let Some(sb) = &self.supabase {
            let _ = sb.delete("turso_databases", &format!("id=eq.{}", id)).await;
            if let Some(owner) = owner {
                let _ = sb.delete_db(&owner, id).await;
            }
        } else {
            self.save_manifest()?;
        }
        tracing::info!("Deleted database: {}", id);
        Ok(())
    }

    pub fn list_databases(&self, owner: Option<&str>) -> Vec<(String, DatabaseEntry)> {
        self.databases
            .iter()
            .filter(|entry| {
                if let Some(owner) = owner {
                    entry.value().1.owner == owner
                } else {
                    true
                }
            })
            .map(|entry| (entry.key().clone(), entry.value().1.clone()))
            .collect()
    }

    pub fn get_db_owner(&self, db_id: &str) -> Option<String> {
        self.databases.get(db_id).map(|e| e.value().1.owner.clone())
    }
}
