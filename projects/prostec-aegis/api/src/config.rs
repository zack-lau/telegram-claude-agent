use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    pub aws_region: String,
    pub dynamodb_table_prefix: String,
    pub s3_delivery_bucket: String,
    pub cognito_user_pool_id: String,
    pub cognito_client_id: String,
    pub cognito_jwks_uri: String,
    pub secrets_manager_config_arn: String,
    pub kms_oauth_tokens_key_id: String,
    /// Secrets Manager ARN for Google OAuth app credentials (client_id + client_secret JSON).
    pub google_oauth_client_id_secret_arn: String,
    /// Secrets Manager ARN for Microsoft OAuth app credentials (client_id + client_secret JSON).
    pub microsoft_oauth_client_id_secret_arn: String,
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
