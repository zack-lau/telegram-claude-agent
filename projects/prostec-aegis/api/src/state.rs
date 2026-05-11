use anyhow::Result;
use aws_config::SdkConfig;
use aws_sdk_dynamodb::Client as DdbClient;
use aws_sdk_secretsmanager::Client as SmClient;
use aws_sdk_cognitoidentityprovider::Client as CognitoClient;
use aws_sdk_kms::Client as KmsClient;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::crypto::jwt::JwtValidator;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pub cfg: Config,
    pub ddb: DdbClient,
    pub sm: SmClient,
    pub cognito: CognitoClient,
    pub kms: KmsClient,
    pub jwt: JwtValidator,
    pub http: reqwest::Client,
}

impl AppState {
    pub async fn new(cfg: Config, aws_cfg: &SdkConfig) -> Result<Self> {
        // Fix H11 — all reqwest clients must carry explicit timeouts.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("build http client: {}", e))?;

        // Per ADR 0004 the JWT validator accepts an allowlist of all configured Cognito
        // app client IDs (legacy single-client + per-platform mobile/web/agent). The
        // legacy id is kept first for back-compat; per-platform empties are filtered
        // out by the validator constructor. Same allowlist is used downstream by
        // `classify_client` to derive ClientType from the signed `client_id` claim.
        let client_ids = [
            cfg.cognito_client_id.as_str(),
            cfg.cognito_client_id_mobile.as_str(),
            cfg.cognito_client_id_web.as_str(),
            cfg.cognito_client_id_agent.as_str(),
        ];
        let jwt = JwtValidator::new(
            &cfg.cognito_jwks_uri,
            &cfg.cognito_user_pool_id,
            &client_ids,
        )
        .await?;
        Ok(Self(Arc::new(Inner {
            ddb: DdbClient::new(aws_cfg),
            sm: SmClient::new(aws_cfg),
            cognito: CognitoClient::new(aws_cfg),
            kms: KmsClient::new(aws_cfg),
            jwt,
            http,
            cfg,
        })))
    }

    pub fn cfg(&self) -> &Config {
        &self.0.cfg
    }

    pub fn ddb(&self) -> &DdbClient {
        &self.0.ddb
    }

    pub fn sm(&self) -> &SmClient {
        &self.0.sm
    }

    pub fn cognito(&self) -> &CognitoClient {
        &self.0.cognito
    }

    pub fn kms(&self) -> &KmsClient {
        &self.0.kms
    }

    pub fn jwt(&self) -> &JwtValidator {
        &self.0.jwt
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.0.http
    }
}
