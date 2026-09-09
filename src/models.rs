use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct DatabaseResponse {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub sql: String,
}

#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct GoogleTokenRequest {
    pub id_token: String,
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub plan: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ClientUserInfo {
    pub id: String,
    pub username: String,
    pub plan: String,
    pub created_at: String,
    pub api_key: String,
    pub database_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct PlanUpdateRequest {
    pub plan: String,
}

#[derive(Debug, Serialize)]
pub struct SetupResponse {
    pub database_id: String,
    pub database_name: String,
    pub schema: Vec<String>,
    pub seeded: bool,
}

#[derive(Debug, Serialize)]
pub struct RateLimitInfo {
    pub remaining: u64,
    pub limit: u64,
    pub window_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub retry: Option<crate::webhooks::RetryPolicy>,
}

/// Partial webhook update. `secret`/`retry` use a double option: an absent field leaves it
/// unchanged, an explicit `null` clears it, and a value sets it. `url`/`events`/`headers`
/// replace the current value when present.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateWebhookRequest {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "optional_field::clearable_string")]
    pub secret: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_field::clearable_retry")]
    pub retry: Option<Option<crate::webhooks::RetryPolicy>>,
}

/// Double-option helpers so a PATCH field can distinguish "absent" (`None`, leave
/// unchanged) from an explicit JSON `null` (`Some(None)`, clear it) from a value
/// (`Some(Some(v))`, set it). Plain `Option<Option<T>>` treats `null` as absent.
pub mod optional_field {
    use serde::Deserialize;

    pub fn clearable_string<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        clearable(d)
    }

    pub fn clearable_retry<'de, D>(
        d: D,
    ) -> Result<Option<Option<crate::webhooks::RetryPolicy>>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        clearable(d)
    }

    fn clearable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de>,
    {
        use std::marker::PhantomData;

        struct Double<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Double<T> {
            type Value = Option<Option<T>>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a value, an explicit null, or an absent field")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Some(None))
            }

            fn visit_some<D2>(self, d: D2) -> Result<Self::Value, D2::Error>
            where
                D2: serde::Deserializer<'de>,
            {
                T::deserialize(d).map(|v| Some(Some(v)))
            }
        }

        d.deserialize_option(Double(PhantomData))
    }
}

#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub retry: Option<crate::webhooks::RetryPolicy>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct SyncResponse {
    pub applied: usize,
    pub rows_affected: u64,
}
