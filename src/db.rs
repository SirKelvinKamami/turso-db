use dashmap::DashMap;
use std::path::Path;
use std::sync::Arc;
use turso::Builder;
use uuid::Uuid;

#[derive(Clone)]
pub struct DatabaseEntry {
    pub owner: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct DatabaseManager {
    data_dir: String,
    databases: Arc<DashMap<String, (turso::Database, DatabaseEntry)>>,
}

impl DatabaseManager {
    pub async fn new(data_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            data_dir: data_dir.to_string(),
            databases: Arc::new(DashMap::new()),
        })
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
