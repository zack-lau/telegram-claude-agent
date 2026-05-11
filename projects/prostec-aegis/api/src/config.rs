use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    pub aws_region: String,
    pub dynamodb_table_prefix: String,
    pub cognito_user_pool_id: String,
    /// Single-client legacy. Use `cognito_client_id_mobile` / `_web` / `_agent` (ADR 0004)
    /// for new deployments. Kept for backward compatibility during migration.
    pub cognito_client_id: String,
    /// Per-platform Cognito app clients (ADR 0004). When the JWT's `client_id` claim
    /// matches one of these, `ClientType` is set deterministically from the server side
    /// instead of trusting an `X-Aegis-Client-Type` header. Empty defaults are accepted
    /// during migration; once populated, header-based classification is overridden.
    #[serde(default)]
    pub cognito_client_id_mobile: String,
    #[serde(default)]
    pub cognito_client_id_web: String,
    #[serde(default)]
    pub cognito_client_id_agent: String,
    pub cognito_jwks_uri: String,
    pub secrets_manager_config_arn: String,
    pub kms_oauth_tokens_key_id: String,
    /// Secrets Manager ARN for Google OAuth app credentials (client_id + client_secret JSON).
    pub google_oauth_client_id_secret_arn: String,
    /// Secrets Manager ARN for Microsoft OAuth app credentials (client_id + client_secret JSON).
    pub microsoft_oauth_client_id_secret_arn: String,
    /// Fix H10 — explicit CORS allowed origins. Default: empty (no CORS).
    /// Must be explicitly configured in production (e.g. CORS_ALLOWED_ORIGINS="https://app.example.com").
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Canonical API base URL used for DPoP `htu` validation (H-3).
    /// Set to the public-facing origin, e.g. `https://api.aegis.example`.
    /// When empty, DPoP falls back to reconstructing the URL from proxy headers
    /// (spoofable — acceptable only in local dev, never in production).
    #[serde(default)]
    pub api_base_url: String,
}

fn default_port() -> u16 {
    8080
}

impl Config {
    pub fn load() -> Result<Self> {
        let cfg = config::Config::builder()
            .add_source(config::Environment::default().separator("__"))
            .build()?
            .try_deserialize()?;
        Ok(cfg)
    }

    pub fn table(&self, name: &str) -> String {
        format!("{}-{}", self.dynamodb_table_prefix, name)
    }
}
