use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use turso::Builder;
use uuid::Uuid;

use crate::supabase::Supabase;

fn value_to_string(value: turso::Value) -> String {
    match value {
        turso::Value::Null => "NULL".to_string(),
        turso::Value::Integer(n) => n.to_string(),
        turso::Value::Real(f) => f.to_string(),
        turso::Value::Text(s) => s,
        turso::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

/// Splits SQL text into individual statements on top-level semicolons, keeping
/// semicolons inside string literals, quoted identifiers, and comments intact.
fn split_sql(sql: &str) -> Vec<String> {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Normal,
        Single,
        Double,
        Backtick,
        Bracket,
        LineComment,
        BlockComment,
    }

    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut mode = Mode::Normal;

    while let Some(c) = chars.next() {
        match mode {
            Mode::Normal => match c {
                '\'' => {
                    mode = Mode::Single;
                    current.push(c);
                }
                '"' => {
                    mode = Mode::Double;
                    current.push(c);
                }
                '`' => {
                    mode = Mode::Backtick;
                    current.push(c);
                }
                '[' => {
                    mode = Mode::Bracket;
                    current.push(c);
                }
                '-' if chars.peek() == Some(&'-') => {
                    mode = Mode::LineComment;
                    current.push(c);
                    current.push('-');
                    chars.next();
                }
                '/' if chars.peek() == Some(&'*') => {
                    mode = Mode::BlockComment;
                    current.push(c);
                    current.push('*');
                    chars.next();
                }
                ';' => {
                    let stmt = current.trim().to_string();
                    if !stmt.is_empty() {
                        statements.push(stmt);
                    }
                    current.clear();
                }
                _ => current.push(c),
            },
            Mode::Single => {
                current.push(c);
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        current.push('\'');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Double => {
                current.push(c);
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        current.push('"');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Backtick => {
                current.push(c);
                if c == '`' {
                    if chars.peek() == Some(&'`') {
                        current.push('`');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Bracket => {
                current.push(c);
                if c == ']' {
                    if chars.peek() == Some(&']') {
                        current.push(']');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::LineComment => {
                current.push(c);
                if c == '\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                current.push(c);
                if c == '*' && chars.peek() == Some(&'/') {
                    current.push('/');
                    chars.next();
                    mode = Mode::Normal;
                }
            }
        }
    }

    let stmt = current.trim().to_string();
    if !stmt.is_empty() {
        statements.push(stmt);
    }
    statements
}

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
    pub async fn new(
        data_dir: &str,
        supabase: Option<Supabase>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
                    if !Path::new(&path).exists() {
                        continue;
                    }
                    match Builder::new_local(&path).build().await {
                        Ok(db) => {
                            self.databases.insert(
                                entry.id.clone(),
                                (
                                    db,
                                    DatabaseEntry {
                                        owner: entry.owner.clone(),
                                        name: entry.name.clone(),
                                        created_at: entry.created_at.clone(),
                                    },
                                ),
                            );
                            tracing::info!("Reloaded database: {} ({})", entry.name, entry.id);
                        }
                        Err(e) => tracing::warn!("Failed to reload database {}: {}", entry.id, e),
                    }
                }
            }
        }

        let known: Vec<String> = self
            .databases
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
            else {
                continue;
            };
            if !file_name.ends_with(".db") || file_name == "auth.db" || file_name == "users.json" {
                continue;
            }
            let id = file_name.trim_end_matches(".db").to_string();
            if known.contains(&id) {
                continue;
            }
            let db_path = path.to_string_lossy().into_owned();
            match Builder::new_local(&db_path).build().await {
                Ok(db) => {
                    self.databases.insert(
                        id.clone(),
                        (
                            db,
                            DatabaseEntry {
                                owner: "admin".to_string(),
                                name: format!("recovered-{}", id),
                                created_at: chrono::Utc::now().to_rfc3339(),
                            },
                        ),
                    );
                    tracing::warn!("Recovered orphan database file {} as owner admin", id);
                    recovered = true;
                }
                Err(e) => tracing::warn!("Failed to recover database file {}: {}", file_name, e),
            }
        }
        if recovered {
            self.save_manifest()?;
        }
        Ok(())
    }

    async fn load_from_supabase(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sb = self.supabase.as_ref().unwrap();
        let rows = sb.rows("turso_databases", "").await?;
        let mut ids: Vec<String> = Vec::new();
        for row in rows {
            let id = row
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let owner = row
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = row
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            ids.push(id.clone());
            let path = format!("{}/{}.db", self.data_dir, id);
            if !Path::new(&path).exists() {
                match sb.download_db(&owner, &id).await {
                    Ok(bytes) => {
                        tokio::fs::write(&path, bytes).await?;
                        tracing::info!("Restored database {} ({}) from storage", name, id);
                    }
                    Err(e) => {
                        tracing::warn!("No stored backup for {} ({}): {}", name, id, e);
                        Builder::new_local(&path).build().await?;
                    }
                }
            }
            match Builder::new_local(&path).build().await {
                Ok(db) => {
                    self.databases.insert(
                        id.clone(),
                        (
                            db,
                            DatabaseEntry {
                                owner,
                                name,
                                created_at,
                            },
                        ),
                    );
                    tracing::info!("Loaded database from registry: {}", id);
                }
                Err(e) => tracing::warn!("Failed to open database {}: {}", id, e),
            }
        }

        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
            else {
                continue;
            };
            if !file_name.ends_with(".db") || file_name == "auth.db" {
                continue;
            }
            let id = file_name.trim_end_matches(".db").to_string();
            if !ids.contains(&id) {
                let _ = std::fs::remove_file(&path);
                tracing::warn!("Removed orphan database file not in registry: {}", id);
            }
        }
        Ok(())
    }

    fn save_manifest(&self) -> Result<(), Box<dyn std::error::Error>> {
        let entries: Vec<ManifestEntry> = self
            .databases
            .iter()
            .map(|entry| {
                let id = entry.key().clone();
                let meta = entry.value().1.clone();
                ManifestEntry {
                    id,
                    owner: meta.owner,
                    name: meta.name,
                    created_at: meta.created_at,
                }
            })
            .collect();
        let raw = serde_json::to_string_pretty(&entries)?;
        std::fs::write(&self.manifest_path, raw)?;
        Ok(())
    }

    pub async fn create_database(
        &self,
        name: &str,
        owner: &str,
    ) -> Result<(String, DatabaseEntry), Box<dyn std::error::Error>> {
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
            if let Err(e) = sb
                .insert(
                    "turso_databases",
                    serde_json::json!({
                        "id": id,
                        "name": entry.name,
                        "owner": entry.owner,
                        "created_at": entry.created_at,
                    }),
                )
                .await
            {
                tracing::error!("Failed to persist database registry row: {}", e);
            }
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    if let Err(e) = sb.upload_db(&entry.owner, &id, bytes).await {
                        tracing::error!("Failed to upload new database file: {}", e);
                    }
                }
                Err(e) => tracing::error!("Failed to read new database file: {}", e),
            }
        } else {
            self.save_manifest()?;
        }
        tracing::info!("Created database: {} (owner: {}) at {}", name, owner, path);
        Ok((id, entry))
    }

    pub async fn get_database(
        &self,
        id: &str,
    ) -> Result<(turso::Database, DatabaseEntry), Box<dyn std::error::Error>> {
        self.databases
            .get(id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("Database {} not found", id).into())
    }

    async fn persist_db(&self, db_id: &str, owner: &str) -> Result<(), Box<dyn std::error::Error>> {
        let Some(sb) = &self.supabase else {
            return Ok(());
        };
        let path = format!("{}/{}.db", self.data_dir, db_id);
        if let Some(entry) = self.databases.get(db_id)
            && let Ok(conn) = entry.value().0.connect()
        {
            let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE);", ()).await;
        }
        let bytes = tokio::fs::read(&path).await?;
        if let Err(e) = sb.upload_db(owner, db_id, bytes).await {
            tracing::error!("Failed to persist database {} to storage: {}", db_id, e);
        }
        Ok(())
    }

    pub async fn execute(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let statements = split_sql(sql);
        if statements.is_empty() {
            return Ok("0 rows affected".to_string());
        }
        let mut total: u64 = 0;
        for stmt in &statements {
            total += conn.execute(stmt, ()).await?;
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        if statements.len() == 1 {
            Ok(format!("{} rows affected", total))
        } else {
            Ok(format!(
                "ran {} statements, {} rows affected",
                statements.len(),
                total
            ))
        }
    }

    pub async fn query_with_columns(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<String>>), Box<dyn std::error::Error>> {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let mut rows = conn.query(sql, ()).await?;
        let columns: Vec<String> = rows
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let mut row_data = Vec::new();
            for i in 0..row.column_count() {
                row_data.push(
                    row.get_value(i)
                        .map(value_to_string)
                        .unwrap_or_else(|_| "?".to_string()),
                );
            }
            results.push(row_data);
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok((columns, results))
    }

    pub async fn query(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;
        let mut rows = conn.query(sql, ()).await?;
        let mut results = Vec::new();

        while let Some(row) = rows.next().await? {
            let mut row_data = Vec::new();
            for i in 0..row.column_count() {
                let value = row
                    .get_value(i)
                    .map(value_to_string)
                    .unwrap_or_else(|_| "?".to_string());
                row_data.push(value);
            }
            results.push(row_data);
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok(results)
    }

    pub async fn run_statement(
        &self,
        db_id: &str,
        sql: &str,
        p: turso::params::Params,
    ) -> Result<
        (Vec<(String, Option<String>)>, Vec<Vec<turso::Value>>, u64),
        Box<dyn std::error::Error>,
    > {
        let (db, entry) = self.get_database(db_id).await?;
        let conn = db.connect()?;

        let head = sql.trim_start().to_ascii_lowercase();
        let is_query = head.starts_with("select")
            || head.starts_with("with")
            || head.starts_with("pragma")
            || head.starts_with("explain")
            || head.starts_with("values");

        let mut cols: Vec<(String, Option<String>)> = Vec::new();
        let mut rows_out: Vec<Vec<turso::Value>> = Vec::new();
        let affected: u64;

        if is_query {
            let mut rows = conn.query(sql, p).await?;
            for c in rows.columns() {
                cols.push((c.name().to_string(), c.decl_type().map(|s| s.to_string())));
            }
            while let Some(row) = rows.next().await? {
                let mut r = Vec::with_capacity(row.column_count());
                for i in 0..row.column_count() {
                    r.push(row.get_value(i)?);
                }
                rows_out.push(r);
            }
            affected = 0;
        } else {
            affected = conn.execute(sql, p).await?;
            cols.push(("changes".to_string(), Some("integer".to_string())));
            rows_out.push(vec![turso::Value::Integer(affected as i64)]);
            self.persist_db(db_id, &entry.owner).await?;
        }

        Ok((cols, rows_out, affected))
    }

    pub async fn delete_database(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let owner = self.databases.get(id).map(|e| e.value().1.owner.clone());
        self.databases.remove(id);
        let base = format!("{}/{}", self.data_dir, id);
        for suffix in [".db", ".db-wal", ".db-shm"] {
            let f = format!("{}{}", base, suffix);
            if Path::new(&f).exists() {
                let _ = std::fs::remove_file(&f);
            }
        }
        if let Some(sb) = &self.supabase {
            if let Err(e) = sb.delete("turso_databases", &format!("id=eq.{}", id)).await {
                tracing::error!("Failed to delete registry row for {}: {}", id, e);
            }
            if let Some(owner) = owner
                && let Err(e) = sb.delete_db(&owner, id).await
            {
                tracing::error!("Failed to delete storage object for {}: {}", id, e);
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

#[cfg(test)]
mod tests {
    use super::split_sql;

    #[test]
    fn splits_multiple_statements() {
        let stmts = split_sql("CREATE TABLE t (a TEXT); INSERT INTO t VALUES ('x');");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE t (a TEXT)");
        assert_eq!(stmts[1], "INSERT INTO t VALUES ('x')");
    }

    #[test]
    fn keeps_semicolons_inside_strings() {
        let stmts = split_sql("INSERT INTO t VALUES ('a;b'); SELECT 1;");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "INSERT INTO t VALUES ('a;b')");
    }

    #[test]
    fn ignores_semicolons_in_comments() {
        let stmts = split_sql("-- hello; world\nSELECT 1; /* cmt; */ SELECT 2");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "-- hello; world\nSELECT 1");
        assert_eq!(stmts[1], "/* cmt; */ SELECT 2");
    }

    #[test]
    fn handles_quoted_identifiers_and_escapes() {
        let stmts = split_sql("CREATE TABLE \"weird;name\" (a TEXT); SELECT 'it''s; ok' AS x");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE \"weird;name\" (a TEXT)");
        assert_eq!(stmts[1], "SELECT 'it''s; ok' AS x");
    }

    #[test]
    fn handles_backtick_and_bracket_identifiers() {
        let stmts = split_sql("CREATE TABLE `t;a` (b TEXT); SELECT 1 FROM [x;y]");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE `t;a` (b TEXT)");
        assert_eq!(stmts[1], "SELECT 1 FROM [x;y]");
    }

    #[test]
    fn drops_empty_statements() {
        let stmts = split_sql(";; SELECT 1 ;;");
        assert_eq!(stmts, vec!["SELECT 1"]);
    }

    #[test]
    fn single_statement_without_terminator() {
        let stmts = split_sql("SELECT 1");
        assert_eq!(stmts, vec!["SELECT 1"]);
    }

    #[test]
    fn empty_input_yields_no_statements() {
        assert!(split_sql("   \n\t ").is_empty());
        assert!(split_sql("").is_empty());
    }
}
