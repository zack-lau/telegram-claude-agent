use anyhow::Result;
use aws_sdk_dynamodb::{types::AttributeValue, Client as DdbClient};
use uuid::Uuid;

use crate::models::key_directory::KeyDirectoryRecord;

pub struct KeyDirectoryStore<'a> {
    ddb: &'a DdbClient,
    table: String,
}

impl<'a> KeyDirectoryStore<'a> {
    pub fn new(ddb: &'a DdbClient, table_prefix: &str) -> Self {
        Self {
            ddb,
            table: format!("{}-key-directory", table_prefix),
        }
    }

    pub async fn get(&self, recipient_id: Uuid) -> Result<Option<KeyDirectoryRecord>> {
        let result = self.ddb
            .get_item()
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb get error: {}", e))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };

        let record = KeyDirectoryRecord {
            recipient_id,
            kem_pk_b64: item.get("kem_pk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            ec_pk_b64: item.get("ec_pk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            ecdsa_pk_b64: item.get("ecdsa_pk").and_then(|v| v.as_s().ok()).cloned(),
            key_version: item.get("key_version")
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1),
            expires_at: item.get("expires_at")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            signature_b64: item.get("signature").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            signer_key_id: item.get("signer_key_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            enc_sk_b64: item.get("enc_sk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            enc_sk_recovery_b64: item.get("enc_sk_recovery").and_then(|v| v.as_s().ok()).cloned(),
            created_at: item.get("created_at")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
        };

        Ok(Some(record))
    }

    pub async fn put(&self, record: &KeyDirectoryRecord) -> Result<()> {
        let mut builder = self.ddb
            .put_item()
            .table_name(&self.table)
            .item("recipient_id", AttributeValue::S(record.recipient_id.to_string()))
            .item("kem_pk", AttributeValue::S(record.kem_pk_b64.clone()))
            .item("ec_pk", AttributeValue::S(record.ec_pk_b64.clone()))
            .item("key_version", AttributeValue::N(record.key_version.to_string()))
            .item("expires_at", AttributeValue::S(record.expires_at.to_rfc3339()))
            .item("signature", AttributeValue::S(record.signature_b64.clone()))
            .item("signer_key_id", AttributeValue::S(record.signer_key_id.clone()))
            .item("enc_sk", AttributeValue::S(record.enc_sk_b64.clone()))
            .item("created_at", AttributeValue::S(record.created_at.to_rfc3339()));

        if let Some(ref ecdsa_pk) = record.ecdsa_pk_b64 {
            builder = builder.item("ecdsa_pk", AttributeValue::S(ecdsa_pk.clone()));
        }

        if let Some(ref recovery) = record.enc_sk_recovery_b64 {
            builder = builder.item("enc_sk_recovery", AttributeValue::S(recovery.clone()));
        }

        builder.send().await.map_err(|e| anyhow::anyhow!("dynamodb put error: {}", e))?;
        Ok(())
    }
}
