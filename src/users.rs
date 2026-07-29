use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct UserStore {
    users: Arc<DashMap<String, User>>,
    file_path: String,
}

impl UserStore {
    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn new(data_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let file_path = format!("{}/users.json", data_dir);
        let store = Arc::new(DashMap::new());

        if Path::new(&file_path).exists() {
            let content = std::fs::read_to_string(&file_path)?;
            let users: Vec<User> = serde_json::from_str(&content)?;
            for user in users {
                store.insert(user.username.clone(), user);
            }
            tracing::info!("Loaded {} users from {}", store.len(), file_path);
        }

        Ok(Self { users: store, file_path })
    }

    fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let users: Vec<User> = self.users.iter().map(|e| e.value().clone()).collect();
        let content = serde_json::to_string_pretty(&users)?;
        std::fs::write(&self.file_path, content)?;
        Ok(())
    }

    pub fn get_user(&self, username: &str) -> Option<User> {
        self.users.get(username).map(|e| e.value().clone())
    }

    pub fn list_users(&self) -> Vec<User> {
        self.users.iter().map(|e| e.value().clone()).collect()
    }

    pub fn create_user(&self, username: &str, password: &str) -> Result<User, String> {
        if self.users.contains_key(username) {
            return Err("Username already exists".into());
        }

        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| e.to_string())?
            .to_string();

        let user = User {
            id: Uuid::new_v4().to_string(),
            username: username.to_string(),
            password_hash: hash,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        self.users.insert(username.to_string(), user.clone());
        self.save().map_err(|e| e.to_string())?;
        Ok(user)
    }

    pub fn delete_user(&self, username: &str) -> Result<(), String> {
        self.users.remove(username);
        self.save().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn verify_password(&self, username: &str, password: &str) -> Result<User, String> {
        let user = self.users.get(username).ok_or_else(|| "User not found".to_string())?;
        let parsed_hash = PasswordHash::new(&user.password_hash).map_err(|e| e.to_string())?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| "Invalid password".to_string())?;
        Ok(user.value().clone())
    }
}
