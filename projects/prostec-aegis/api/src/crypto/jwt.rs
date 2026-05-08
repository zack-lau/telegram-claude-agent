// Cognito JWT access token validation.
// Stateless: verify signature against Cognito JWKS, check exp and iss.
// No DynamoDB hit on the hot path — epoch check is a separate middleware layer.

use anyhow::{bail, Result};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CognitoClaims {
    pub sub: String,
    pub iss: String,
    pub exp: u64,
    pub iat: u64,
    pub token_use: String,
    /// Cognito groups the user belongs to.
    #[serde(rename = "cognito:groups", default)]
    pub groups: Vec<String>,
}

/// JWKS key cache — refreshed lazily on key ID miss.
#[derive(Clone)]
pub struct JwtValidator {
    jwks_uri: String,
    expected_issuer: String,
    keys: Arc<RwLock<HashMap<String, DecodingKey>>>,
}

impl JwtValidator {
    pub async fn new(jwks_uri: &str, user_pool_id: &str) -> Result<Self> {
        let region = user_pool_id
            .split('_')
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid user pool id format"))?;
        let expected_issuer = format!("https://cognito-idp.{}.amazonaws.com/{}", region, user_pool_id);
        let mut v = Self {
            jwks_uri: jwks_uri.to_owned(),
            expected_issuer,
            keys: Arc::new(RwLock::new(HashMap::new())),
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
                self.refresh_keys().await?;
                let r = self.keys.read().await;
                r.get(&kid)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown jwt kid after refresh"))?
            }
        };

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.expected_issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);

        let data = decode::<CognitoClaims>(token, &key, &validation)
            .map_err(|e| anyhow::anyhow!("jwt validation failed: {}", e))?;

        if data.claims.token_use != "access" {
            bail!("expected access token, got {}", data.claims.token_use);
        }

        Ok(data.claims)
    }

    async fn refresh_keys(&self) -> Result<()> {
        let resp = reqwest::get(&self.jwks_uri)
            .await
            .map_err(|e| anyhow::anyhow!("failed to fetch jwks: {}", e))?;
        let jwks: serde_json::Value = resp.json().await?;

        let mut keys = self.keys.write().await;
        keys.clear();
        for key in jwks["keys"].as_array().unwrap_or(&vec![]) {
            let kid = key["kid"].as_str().unwrap_or("").to_owned();
            if kid.is_empty() {
                continue;
            }
            let n = key["n"].as_str().unwrap_or("");
            let e = key["e"].as_str().unwrap_or("");
            if let Ok(dk) = DecodingKey::from_rsa_components(n, e) {
                keys.insert(kid, dk);
            }
        }
        Ok(())
    }
}
