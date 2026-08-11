use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use turso::Builder;
use uuid::Uuid;

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
}

impl DatabaseManager {
    pub async fn new(data_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let manager = Self {
            data_dir: data_dir.to_string(),
            manifest_path: format!("{}/databases.json", data_dir),
            databases: Arc::new(DashMap::new()),
        };
        manager.load_manifest().await?;
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
        self.save_manifest()?;
        tracing::info!("Created database: {} (owner: {}) at {}", name, owner, path);
        Ok((id, entry))
    }

    pub async fn get_database(&self, id: &str) -> Result<(turso::Database, DatabaseEntry), Box<dyn std::error::Error>> {
        self.databases
            .get(id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("Database {} not found", id).into())
    }

    pub async fn execute(&self, db_id: &str, sql: &str) -> Result<String, Box<dyn std::error::Error>> {
        let (db, _) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let result = conn.execute(sql, ()).await?;
        Ok(format!("{} rows affected", result))
    }

    pub async fn query(&self, db_id: &str, sql: &str) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
        let (db, _) = self.get_database(db_id).await?;
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
        Ok(results)
    }

    pub async fn delete_database(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.databases.remove(id);
        let path = format!("{}/{}.db", self.data_dir, id);
        if Path::new(&path).exists() {
            std::fs::remove_file(path)?;
        }
        self.save_manifest()?;
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
