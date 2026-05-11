// Cognito JWT access token validation.
// Stateless: verify signature against Cognito JWKS, check exp and iss.
// No DynamoDB hit on the hot path — epoch check is a separate middleware layer.

use anyhow::{bail, Result};
use base64::Engine;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// After a JWKS fetch failure, refuse to retry for this window (Q-M2 thundering herd).
const REFRESH_ERROR_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CognitoClaims {
    pub sub: String,
    pub iss: String,
    pub exp: u64,
    pub iat: u64,
    pub token_use: String,
    /// Cognito access tokens use `client_id` (not `aud`) to identify the calling app client.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Cognito groups the user belongs to.
    #[serde(rename = "cognito:groups", default)]
    pub groups: Vec<String>,
}

/// JWKS key cache — refreshed lazily on key ID miss.
#[derive(Clone)]
pub struct JwtValidator {
    jwks_uri: String,
    expected_issuer: String,
    /// Allowlist of acceptable Cognito app client IDs (ADR 0004 — one per platform:
    /// mobile / web / agent — plus the legacy single-client ID during migration).
    /// Tokens whose `client_id` claim is NOT in this set are rejected. Empty entries
    /// are filtered out so an unconfigured per-platform field doesn't accidentally
    /// match an empty string token claim.
    expected_client_ids: Vec<String>,
    keys: Arc<RwLock<HashMap<String, DecodingKey>>>,
    /// Shared HTTP client — reused across JWKS refresh calls to avoid TCP/TLS churn.
    client: reqwest::Client,
    /// Single-flight guard for `refresh_keys`. Concurrent kid-misses serialize on this
    /// mutex so only one task fetches JWKS while others wait for the result, preventing
    /// network churn and rate-limit exposure (Qwen Round 6 crypto MEDIUM).
    refresh_lock: Arc<Mutex<()>>,
    /// Tracks when the last JWKS refresh error occurred. Tasks that acquire refresh_lock
    /// during the cooldown window skip the network call and fail fast, preventing a
    /// thundering herd from hammering a down JWKS endpoint (Q-M2).
    refresh_error_at: Arc<Mutex<Option<Instant>>>,
}

impl JwtValidator {
    /// Construct from one or more accepted Cognito app client IDs. The first is
    /// typically the legacy single-client ID; remaining entries are the per-platform
    /// IDs from ADR 0004 (mobile / web / agent). Empty strings are filtered out.
    /// At least one non-empty client_id MUST be provided.
    pub async fn new(jwks_uri: &str, user_pool_id: &str, client_ids: &[&str]) -> Result<Self> {
        // Reject non-HTTPS JWKS URIs at construction. JWKS over plaintext is a MITM
        // pivot for forging JWTs (Qwen Round 2 crypto finding).
        if !jwks_uri.starts_with("https://") {
            bail!("jwks_uri must use https:// scheme; got: {}", jwks_uri);
        }
        let expected_client_ids: Vec<String> = client_ids
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| (*s).to_owned())
            .collect();
        if expected_client_ids.is_empty() {
            bail!("at least one non-empty cognito client_id must be provided");
        }
        let region = user_pool_id
            .split('_')
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid user pool id format"))?;
        let expected_issuer = format!("https://cognito-idp.{}.amazonaws.com/{}", region, user_pool_id);
        let client = reqwest::Client::builder()
            .https_only(true) // belt-and-braces: refuse HTTP redirects too
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("build http client: {}", e))?;
        let v = Self {
            jwks_uri: jwks_uri.to_owned(),
            expected_issuer,
            expected_client_ids,
            keys: Arc::new(RwLock::new(HashMap::new())),
            client,
            refresh_lock: Arc::new(Mutex::new(())),
            refresh_error_at: Arc::new(Mutex::new(None)),
        };
        v.refresh_keys().await?;
        Ok(v)
    }

    /// Validate a JWT access token. Returns claims on success.
    pub async fn validate(&self, token: &str) -> Result<CognitoClaims> {
        let header = decode_header(token)
            .map_err(|_| anyhow::anyhow!("invalid jwt header"))?;
        let kid = header.kid.ok_or_else(|| anyhow::anyhow!("jwt missing kid"))?;

        // Try cached key first; refresh on miss.
        let key = {
            let r = self.keys.read().await;
            r.get(&kid).cloned()
        };
        let key = match key {
            Some(k) => k,
            None => {
                // Single-flight guard: only one task fetches JWKS at a time. After
                // acquiring the lock, re-check the cache — another task may have
                // refreshed between our miss and our turn (Qwen Round 6 crypto MEDIUM).
                let _guard = self.refresh_lock.lock().await;
                {
                    let r = self.keys.read().await;
                    if let Some(k) = r.get(&kid).cloned() {
                        drop(r);
                        return self.finish_validate(token, &k).await;
                    }
                }
                // Cooldown check: if JWKS fetch failed recently, fail fast rather than
                // hammering a down endpoint on every concurrent miss (Q-M2).
                {
                    let last_err = self.refresh_error_at.lock().await;
                    if let Some(t) = *last_err {
                        if t.elapsed() < REFRESH_ERROR_COOLDOWN {
                            bail!("jwks refresh in cooldown after recent failure");
                        }
                    }
                }
                match self.refresh_keys().await {
                    Ok(()) => {}
                    Err(e) => {
                        *self.refresh_error_at.lock().await = Some(Instant::now());
                        bail!("jwks refresh failed: {}", e);
                    }
                }
                let r = self.keys.read().await;
                r.get(&kid)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown jwt kid after refresh"))?
            }
        };
        self.finish_validate(token, &key).await
    }

    async fn finish_validate(&self, token: &str, key: &DecodingKey) -> Result<CognitoClaims> {

        let mut validation = Validation::new(Algorithm::RS256);
        // Explicitly lock the allowed algorithms list to RS256. Defensive against
        // any future jsonwebtoken default change that might broaden algorithm acceptance
        // (Qwen Round 3 crypto MEDIUM).
        validation.algorithms = vec![Algorithm::RS256];
        validation.set_issuer(&[&self.expected_issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);

        let data = decode::<CognitoClaims>(token, &key, &validation)
            .map_err(|e| anyhow::anyhow!("jwt validation failed: {}", e))?;

        if data.claims.token_use != "access" {
            bail!("expected access token, got {}", data.claims.token_use);
        }

        // Cross-pool / wrong-client rejection: the access token's `client_id` claim
        // MUST be in the configured allowlist. ADR 0004: per-platform client IDs
        // distinguish mobile / web / agent — the JWT carries the issuing client_id,
        // and downstream `classify_client` maps it to a `ClientType`.
        // (Codex Round 7 MEDIUM: validator must accept the same set classify_client
        // keys off, otherwise per-platform tokens fail auth before classification.)
        match data.claims.client_id.as_deref() {
            Some(cid) if self.expected_client_ids.iter().any(|allowed| allowed == cid) => {}
            Some(other) => bail!(
                "jwt client_id {} not in allowlist of {} configured ids",
                other, self.expected_client_ids.len()
            ),
            None => bail!("jwt missing required client_id claim"),
        }

        Ok(data.claims)
    }

    async fn refresh_keys(&self) -> Result<()> {
        // Build the new map completely before acquiring the write lock so we never
        // clear the cache before the replacement is ready.
        let resp = self.client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("failed to fetch jwks: {}", e))?;
        let jwks: serde_json::Value = resp.json().await?;

        let mut new_keys: HashMap<String, DecodingKey> = HashMap::new();
        for key in jwks["keys"].as_array().unwrap_or(&vec![]) {
            let kid = key["kid"].as_str().unwrap_or("").to_owned();
            if kid.is_empty() {
                continue;
            }
            let n = key["n"].as_str().unwrap_or("");
            let e = key["e"].as_str().unwrap_or("");
            // Defensive: reject RSA keys whose modulus is < 2048 bits. Cognito always
            // issues 2048-bit keys; a smaller key in the JWKS would be a misconfiguration
            // or compromise (Qwen Round 6 crypto MEDIUM). The base64url-encoded modulus
            // for a 2048-bit RSA key decodes to exactly 256 bytes; we accept ≥ 256 to
            // also allow 3072/4096-bit upgrades.
            let n_bytes = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(n) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if n_bytes.len() < 256 || n_bytes.len() > 1024 {
                tracing::warn!(kid = %kid, modulus_bytes = n_bytes.len(),
                    "rejecting JWKS key: modulus outside 2048..=8192 bit range (M-5)");
                continue;
            }
            if let Ok(dk) = DecodingKey::from_rsa_components(n, e) {
                new_keys.insert(kid, dk);
            }
        }

        if new_keys.is_empty() {
            tracing::warn!("JWKS refresh returned 0 valid keys — keeping stale cache");
            return Ok(());
        }

        // Atomic swap: write lock acquired only after new map is fully built.
        let mut keys = self.keys.write().await;
        *keys = new_keys;
        Ok(())
    }
}
