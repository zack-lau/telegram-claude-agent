use anyhow::Result;
use aws_sdk_dynamodb::{types::AttributeValue, Client as DdbClient};
use uuid::Uuid;

use crate::models::delivery::DeliveryRecord;

pub struct DeliveryStore<'a> {
    ddb: &'a DdbClient,
    table: String,
}

impl<'a> DeliveryStore<'a> {
    pub fn new(ddb: &'a DdbClient, table_prefix: &str) -> Self {
        // deliveries table will be added to dynamodb.tf — stub here for routing layer
        Self {
            ddb,
            table: format!("{}-deliveries", table_prefix),
        }
    }

    pub async fn get(&self, delivery_id: Uuid, recipient_id: Uuid) -> Result<Option<DeliveryRecord>> {
        let result = self.ddb
            .get_item()
            .table_name(&self.table)
            .key("delivery_id", AttributeValue::S(delivery_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb get error: {}", e))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };

        // Verify recipient is in the delivery's recipient set
        let envelope_header_str = item
            .get("envelope_header")
            .and_then(|v| v.as_s().ok())
            .unwrap_or("");

        // Parse envelope to check recipient membership
        let header: crate::crypto::envelope::EnvelopeHeader = serde_json::from_str(envelope_header_str)
            .map_err(|_| anyhow::anyhow!("malformed envelope header in storage"))?;
        header.validate()
            .map_err(|e| anyhow::anyhow!("invalid envelope header from storage: {}", e))?;

        if !header.recipients.iter().any(|s| s.recipient_id == recipient_id) {
            return Ok(None); // Treat as not found — don't reveal existence
        }

        let record = DeliveryRecord {
            delivery_id,
            content_id: item.get("content_id")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            sender_id: item.get("sender_id")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            sender_key_id: item.get("sender_key_id")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            suite_id: item.get("suite_id")
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            envelope_header: envelope_header_str.to_owned(),
            created_at: item.get("created_at")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            expires_at: item.get("expires_at")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            burn_after_read: item.get("burn_after_read")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false),
            decrypted_at: item.get("decrypted_at")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc)),
            decrypted_by_token_id: item.get("decrypted_by_token_id")
                .and_then(|v| v.as_s().ok())
                .and_then(|s| s.parse().ok()),
        };

        Ok(Some(record))
    }
}
