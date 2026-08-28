use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::Instant;

const GOOGLE_CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const CACHE_TTL_SECS: u64 = 3600;

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kid: String,
    #[serde(rename = "n")]
    pub modulus: String,
    #[serde(rename = "e")]
    pub exponent: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleProfile {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
}

struct JwksCache {
    fetched_at: Instant,
    keys: Vec<Jwk>,
}

static CACHE: Mutex<Option<JwksCache>> = Mutex::new(None);

async fn fetch_jwks() -> Result<Vec<Jwk>, String> {
    let resp = reqwest::Client::new()
        .get(GOOGLE_CERTS_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("google certs fetch failed: {}", resp.status()));
    }
    let jwks: Jwks = resp.json().await.map_err(|e| e.to_string())?;
    Ok(jwks.keys)
}

async fn cached_jwks() -> Result<Vec<Jwk>, String> {
    {
        let guard = CACHE.lock().unwrap();
        if let Some(cache) = guard.as_ref()
            && cache.fetched_at.elapsed().as_secs() < CACHE_TTL_SECS
        {
            return Ok(cache.keys.clone());
        }
    }
    let keys = fetch_jwks().await?;
    *CACHE.lock().unwrap() = Some(JwksCache {
        fetched_at: Instant::now(),
        keys: keys.clone(),
    });
    Ok(keys)
}

pub async fn verify_id_token(
    id_token: &str,
    expected_audience: &str,
) -> Result<GoogleProfile, String> {
    let header = decode_header(id_token).map_err(|e| format!("invalid token: {}", e))?;
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| "token missing kid header".to_string())?;

    let keys = cached_jwks().await?;
    let jwk = keys
        .iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| "unknown signing key".to_string())?;

    let key = DecodingKey::from_rsa_components(&jwk.modulus, &jwk.exponent)
        .map_err(|e| format!("invalid signing key: {}", e))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[expected_audience.to_string()]);
    validation.set_issuer(&["accounts.google.com", "https://accounts.google.com"]);
    validation.set_required_spec_claims(&["aud", "iss", "exp"]);

    let profile = decode::<GoogleProfile>(id_token, &key, &validation)
        .map_err(|e| format!("token verification failed: {}", e))?
        .claims;

    if profile.email.is_empty() {
        return Err("token has no email claim".to_string());
    }
    Ok(profile)
}
