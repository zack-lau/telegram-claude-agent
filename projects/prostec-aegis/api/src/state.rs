use anyhow::Result;
use aws_config::SdkConfig;
use aws_sdk_dynamodb::Client as DdbClient;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_secretsmanager::Client as SmClient;
use aws_sdk_cognitoidentityprovider::Client as CognitoClient;
use aws_sdk_kms::Client as KmsClient;
use std::sync::Arc;

use crate::config::Config;
use crate::crypto::jwt::JwtValidator;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pub cfg: Config,
    pub ddb: DdbClient,
    pub s3: S3Client,
    pub sm: SmClient,
    pub cognito: CognitoClient,
    pub kms: KmsClient,
    pub jwt: JwtValidator,
}

impl AppState {
    pub async fn new(cfg: Config, aws_cfg: &SdkConfig) -> Result<Self> {
        // Takes ownership of cfg; call site passes cfg by value.
        let jwt = JwtValidator::new(&cfg.cognito_jwks_uri, &cfg.cognito_user_pool_id).await?;
        Ok(Self(Arc::new(Inner {
            ddb: DdbClient::new(aws_cfg),
            s3: S3Client::new(aws_cfg),
            sm: SmClient::new(aws_cfg),
            cognito: CognitoClient::new(aws_cfg),
            kms: KmsClient::new(aws_cfg),
            jwt,
            cfg,
        })))
    }

    pub fn cfg(&self) -> &Config {
        &self.0.cfg
    }

    pub fn ddb(&self) -> &DdbClient {
        &self.0.ddb
    }

    pub fn s3(&self) -> &S3Client {
        &self.0.s3
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
}
