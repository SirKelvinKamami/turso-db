use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::plans::Plan;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    #[serde(default = "default_plan")]
    pub plan: String,
    pub created_at: String,
}

fn default_plan() -> String {
    Plan::default().as_str().to_string()
}

#[derive(Clone)]
pub struct UserStore {
    conn: Arc<Mutex<Connection>>,
    db_path: String,
}

impl UserStore {
    pub fn file_path(&self) -> &str {
        &self.db_path
    }

    pub fn new(data_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = format!("{}/auth.db", data_dir);

        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                plan TEXT NOT NULL DEFAULT 'free',
                created_at TEXT NOT NULL
            )",
        )?;

        // Migrate existing databases that lack the plan column
        let has_plan: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(users)")?;
            let mut rows = stmt.query([])?;
            let mut found = false;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "plan" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_plan {
            conn.execute_batch("ALTER TABLE users ADD COLUMN plan TEXT NOT NULL DEFAULT 'free'")?;
            tracing::info!("Migrated users table: added plan column");
        }

        let json_path = format!("{}/users.json", data_dir);
        if Path::new(&json_path).exists() {
            if let Ok(content) = std::fs::read_to_string(&json_path) {
                if let Ok(users) = serde_json::from_str::<Vec<User>>(&content) {
                    let count = users.len();
                    for user in &users {
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO users (id, username, password_hash, plan, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                            rusqlite::params![user.id, user.username, user.password_hash, user.plan, user.created_at],
                        );
                    }
                    tracing::info!("Migrated {} users from users.json to auth.db", count);
                    let _ = std::fs::rename(&json_path, format!("{}/users.json.migrated", data_dir));
                }
            }
        }

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
        tracing::info!("User store initialized with {} users in {}", count, db_path);

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
        Ok(User {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            plan: row.get(3)?,
            created_at: row.get(4)?,
        })
    }

    pub fn get_user(&self, username: &str) -> Option<User> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, username, password_hash, plan, created_at FROM users WHERE username = ?1",
            rusqlite::params![username],
            Self::row_to_user,
        )
        .ok()
    }

    pub fn list_users(&self) -> Vec<User> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, username, password_hash, plan, created_at FROM users ORDER BY created_at")
            .unwrap();
        stmt.query_map([], Self::row_to_user)
            .unwrap()
            .filter_map(|u| u.ok())
            .collect()
    }

    pub fn create_user(&self, username: &str, password: &str) -> Result<User, String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| e.to_string())?
            .to_string();

        let user = User {
            id: Uuid::new_v4().to_string(),
            username: username.to_string(),
            password_hash: hash,
            plan: Plan::default().as_str().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (id, username, password_hash, plan, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![user.id, user.username, user.password_hash, user.plan, user.created_at],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                "Username already exists".to_string()
            } else {
                e.to_string()
            }
        })?;

        Ok(user)
    }

    pub fn set_plan(&self, username: &str, plan: &str) -> Result<User, String> {
        let plan = Plan::from_str(plan);
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE users SET plan = ?1 WHERE username = ?2",
                rusqlite::params![plan.as_str(), username],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("User not found".to_string());
        }
        conn.query_row(
            "SELECT id, username, password_hash, plan, created_at FROM users WHERE username = ?1",
            rusqlite::params![username],
            Self::row_to_user,
        )
        .map_err(|e| e.to_string())
    }

    pub fn delete_user(&self, username: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM users WHERE username = ?1", rusqlite::params![username])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn verify_password(&self, username: &str, password: &str) -> Result<User, String> {
        let conn = self.conn.lock().unwrap();
        let user = conn
            .query_row(
                "SELECT id, username, password_hash, plan, created_at FROM users WHERE username = ?1",
                rusqlite::params![username],
                Self::row_to_user,
            )
            .map_err(|_| "User not found".to_string())?;

        let parsed_hash = PasswordHash::new(&user.password_hash).map_err(|e| e.to_string())?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| "Invalid password".to_string())?;

        Ok(user)
    }
}
