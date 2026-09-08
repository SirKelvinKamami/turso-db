use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
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
    #[serde(default)]
    pub api_key: String,
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

    /// Populate the in-memory cache from Supabase so auth keeps working offline and
    /// avoids a network call on the hot path. Returns the number of users loaded.
    pub async fn load_all(&self) -> usize {
        let users = self.list_users().await;
        let n = users.len();
        for u in users {
            self.mem.insert(u.username.clone(), u);
        }
        n
    }

    fn from_row(v: &serde_json::Value) -> Option<User> {
        Some(User {
            id: v.get("id")?.as_str()?.to_string(),
            username: v.get("username")?.as_str()?.to_string(),
            password_hash: v.get("password_hash")?.as_str()?.to_string(),
            plan: v
                .get("plan")
                .and_then(|p| p.as_str())
                .unwrap_or("free")
                .to_string(),
            api_key: v
                .get("api_key")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: v
                .get("created_at")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    pub async fn find_by_api_key(&self, key: &str) -> Option<User> {
        if key.is_empty() {
            return None;
        }
        if let Some(found) = self.mem.iter().find(|e| e.value().api_key == key) {
            return Some(found.value().clone());
        }
        self.list_users()
            .await
            .into_iter()
            .find(|u| u.api_key == key)
    }

    pub async fn ensure_api_key(&self, username: &str) -> Result<String, String> {
        let user = self
            .get_user(username)
            .await
            .ok_or_else(|| "User not found".to_string())?;
        if !user.api_key.is_empty() {
            return Ok(user.api_key);
        }
        let key = crate::auth::generate_api_key();
        self.set_api_key(username, &key).await?;
        Ok(key)
    }

    pub async fn set_api_key(&self, username: &str, key: &str) -> Result<(), String> {
        if let Some(sb) = &self.supabase {
            sb.update(
                "turso_users",
                &format!("username=eq.{}", username),
                serde_json::json!({ "api_key": key }),
            )
            .await?;
            if let Some(mut u) = self.mem.get_mut(username) {
                u.api_key = key.to_string();
            }
            Ok(())
        } else {
            if let Some(mut u) = self.mem.get_mut(username) {
                u.api_key = key.to_string();
            }
            Ok(())
        }
    }

    pub async fn get_user(&self, username: &str) -> Option<User> {
        if let Some(cached) = self.mem.get(username) {
            return Some(cached.clone());
        }
        if let Some(sb) = &self.supabase {
            let rows = sb
                .rows("turso_users", &format!("&username=eq.{}", username))
                .await
                .ok()?;
            let user = rows.first().and_then(Self::from_row);
            if let Some(u) = &user {
                self.mem.insert(username.to_string(), u.clone());
            }
            user
        } else {
            self.mem.get(username).map(|u| u.clone())
        }
    }

    pub async fn list_users(&self) -> Vec<User> {
        if let Some(sb) = &self.supabase {
            let rows = sb
                .rows("turso_users", "")
                .await
                .map(|rows| rows.iter().filter_map(Self::from_row).collect::<Vec<_>>());
            match rows {
                Ok(users) => users,
                // If Supabase is unreachable, serve the cache so auth doesn't break.
                Err(_) => self.mem.iter().map(|u| u.value().clone()).collect(),
            }
        } else {
            self.mem.iter().map(|u| u.value().clone()).collect()
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
            api_key: crate::auth::generate_api_key(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        if let Some(sb) = &self.supabase {
            let row = serde_json::json!({
                "username": user.username,
                "password_hash": user.password_hash,
                "plan": user.plan,
                "api_key": user.api_key,
            });
            let stored = sb.insert("turso_users", row).await.map_err(|e| {
                if e.contains("duplicate") || e.contains("unique") || e.contains("23505") {
                    "Username already exists".to_string()
                } else {
                    e
                }
            })?;
            let user = Self::from_row(&stored).unwrap_or(user);
            self.mem.insert(user.username.clone(), user.clone());
            Ok(user)
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
            sb.update(
                "turso_users",
                &filter,
                serde_json::json!({ "plan": plan.as_str() }),
            )
            .await?;
            if let Some(mut u) = self.mem.get_mut(username) {
                u.plan = plan.as_str().to_string();
            }
            self.get_user(username)
                .await
                .ok_or_else(|| "User not found".to_string())
        } else {
            let mut user = self
                .mem
                .get_mut(username)
                .ok_or_else(|| "User not found".to_string())?;
            user.plan = plan.as_str().to_string();
            Ok(user.clone())
        }
    }

    pub async fn delete_user(&self, username: &str) -> Result<(), String> {
        if let Some(sb) = &self.supabase {
            sb.delete("turso_users", &format!("username=eq.{}", username))
                .await?;
            self.mem.remove(username);
            Ok(())
        } else {
            self.mem.remove(username);
            Ok(())
        }
    }

    pub async fn verify_password(&self, username: &str, password: &str) -> Result<User, String> {
        let user = self
            .get_user(username)
            .await
            .ok_or_else(|| "User not found".to_string())?;
        let parsed_hash = PasswordHash::new(&user.password_hash).map_err(|e| e.to_string())?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| "Invalid password".to_string())?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_user_and_verifies_password() {
        let store = UserStore::new(None);
        let user = store.create_user("alice", "s3cret").await.unwrap();
        assert_eq!(user.username, "alice");
        assert_ne!(user.password_hash, "s3cret");
        assert!(!user.api_key.is_empty());

        let ok = store.verify_password("alice", "s3cret").await.unwrap();
        assert_eq!(ok.username, "alice");
        assert!(store.verify_password("alice", "wrong").await.is_err());
        assert!(store.verify_password("nobody", "s3cret").await.is_err());
    }

    #[tokio::test]
    async fn duplicate_user_rejected() {
        let store = UserStore::new(None);
        store.create_user("bob", "pass1").await.unwrap();
        let err = store.create_user("bob", "pass2").await.unwrap_err();
        assert_eq!(err, "Username already exists");
    }

    #[tokio::test]
    async fn mem_cache_serves_get_user() {
        let store = UserStore::new(None);
        store.create_user("carol", "pw").await.unwrap();
        // Cache hit path
        let u = store.get_user("carol").await.unwrap();
        assert_eq!(u.username, "carol");
        assert!(store.get_user("missing").await.is_none());
    }

    #[tokio::test]
    async fn ensures_and_rotates_api_key() {
        let store = UserStore::new(None);
        store.create_user("dave", "pw").await.unwrap();
        let key1 = store.ensure_api_key("dave").await.unwrap();
        assert!(!key1.is_empty());
        // Already present: same key returned.
        let key2 = store.ensure_api_key("dave").await.unwrap();
        assert_eq!(key1, key2);

        let new_key = "rotated-123";
        store.set_api_key("dave", new_key).await.unwrap();
        let saved = store.get_user("dave").await.unwrap();
        assert_eq!(saved.api_key, new_key);
        assert_eq!(
            store.find_by_api_key(new_key).await.unwrap().username,
            "dave"
        );
    }

    #[tokio::test]
    async fn set_plan_and_delete() {
        let store = UserStore::new(None);
        store.create_user("erin", "pw").await.unwrap();
        let u = store.set_plan("erin", "Pro").await.unwrap();
        assert_eq!(u.plan, "pro");
        assert_eq!(Plan::from_str(&u.plan), Plan::Pro);

        store.delete_user("erin").await.unwrap();
        assert!(store.get_user("erin").await.is_none());
    }

    #[test]
    fn user_row_deserialization_defaults_plan() {
        let row = serde_json::json!({
            "id": "u1",
            "username": "frank",
            "password_hash": "x",
        });
        let u = UserStore::from_row(&row).unwrap();
        assert_eq!(u.plan, "free");
        assert_eq!(u.api_key, "");
    }
}
