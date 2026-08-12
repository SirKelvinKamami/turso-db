use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::plans::Plan;
use crate::supabase::Supabase;

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
    supabase: Option<Supabase>,
    mem: Arc<DashMap<String, User>>,
}

impl UserStore {
    pub fn file_path(&self) -> &str {
        if self.supabase.is_some() {
            "supabase://public.turso_users"
        } else {
            "memory://turso_users"
        }
    }

    pub fn new(supabase: Option<Supabase>) -> Self {
        Self {
            supabase,
            mem: Arc::new(DashMap::new()),
        }
    }

    fn from_row(v: &serde_json::Value) -> Option<User> {
        Some(User {
            id: v.get("id")?.as_str()?.to_string(),
            username: v.get("username")?.as_str()?.to_string(),
            password_hash: v.get("password_hash")?.as_str()?.to_string(),
            plan: v.get("plan").and_then(|p| p.as_str()).unwrap_or("free").to_string(),
            created_at: v.get("created_at").and_then(|c| c.as_str()).unwrap_or("").to_string(),
        })
    }

    pub async fn get_user(&self, username: &str) -> Option<User> {
        if let Some(sb) = &self.supabase {
            let rows = sb.rows("turso_users", &format!("&username=eq.{}", username)).await.ok()?;
            rows.first().and_then(Self::from_row)
        } else {
            self.mem.get(username).map(|u| u.clone())
        }
    }

    pub async fn list_users(&self) -> Vec<User> {
        if let Some(sb) = &self.supabase {
            sb.rows("turso_users", "").await
                .map(|rows| rows.iter().filter_map(Self::from_row).collect())
                .unwrap_or_default()
        } else {
            self.mem.iter().map(|u| u.clone()).collect()
        }
    }

    pub async fn create_user(&self, username: &str, password: &str) -> Result<User, String> {
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

        if let Some(sb) = &self.supabase {
            let row = serde_json::json!({
                "username": user.username,
                "password_hash": user.password_hash,
                "plan": user.plan,
            });
            let stored = sb.insert("turso_users", row).await.map_err(|e| {
                if e.contains("duplicate") || e.contains("unique") || e.contains("23505") {
                    "Username already exists".to_string()
                } else {
                    e
                }
            })?;
            Ok(Self::from_row(&stored).unwrap_or(user))
        } else {
            if self.mem.contains_key(username) {
                return Err("Username already exists".to_string());
            }
            self.mem.insert(username.to_string(), user.clone());
            Ok(user)
        }
    }

    pub async fn set_plan(&self, username: &str, plan: &str) -> Result<User, String> {
        let plan = Plan::from_str(plan);
        if let Some(sb) = &self.supabase {
            let filter = format!("username=eq.{}", username);
            sb.update("turso_users", &filter, serde_json::json!({ "plan": plan.as_str() })).await?;
            self.get_user(username).await.ok_or_else(|| "User not found".to_string())
        } else {
            let mut user = self.mem.get_mut(username).ok_or_else(|| "User not found".to_string())?;
            user.plan = plan.as_str().to_string();
            Ok(user.clone())
        }
    }

    pub async fn delete_user(&self, username: &str) -> Result<(), String> {
        if let Some(sb) = &self.supabase {
            sb.delete("turso_users", &format!("username=eq.{}", username)).await
        } else {
            self.mem.remove(username);
            Ok(())
        }
    }

    pub async fn verify_password(&self, username: &str, password: &str) -> Result<User, String> {
        let user = self.get_user(username).await.ok_or_else(|| "User not found".to_string())?;
        let parsed_hash = PasswordHash::new(&user.password_hash).map_err(|e| e.to_string())?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| "Invalid password".to_string())?;
        Ok(user)
    }
}
