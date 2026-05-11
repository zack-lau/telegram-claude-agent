use anyhow::Result;
use aws_sdk_dynamodb::{types::AttributeValue, Client as DdbClient};
use uuid::Uuid;

use crate::models::delivery::{DeliveryRecord, StorageProvider};

pub struct DeliveryStore<'a> {
    ddb: &'a DdbClient,
    table: String,
}

impl<'a> DeliveryStore<'a> {
    pub fn new(ddb: &'a DdbClient, table_prefix: &str) -> Self {
        Self {
            ddb,
            table: format!("{}-deliveries", table_prefix),
        }
    }

    pub async fn get(&self, delivery_id: Uuid, recipient_id: Uuid) -> Result<Option<DeliveryRecord>> {
        // Strongly consistent — burn-after-read and revocation must see fresh state.
        let result = self.ddb
            .get_item()
            .consistent_read(true)
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("delivery_id", AttributeValue::S(delivery_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb get error: {}", e))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };

        // Backward compat: rows written before the provider refactor only have cloud_path.
        // Return a clear error rather than a confusing 500 on the missing field.
        if item.contains_key("cloud_path") && !item.contains_key("provider") {
            anyhow::bail!("legacy_cloud_path: delivery predates provider refactor; re-deliver to update");
        }

        let doc_id: Uuid = item
            .get("doc_id")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("missing required field: doc_id"))?;

        let sender_id: Uuid = item
            .get("sender_id")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("missing required field: sender_id"))?;

        let provider: StorageProvider = item
            .get("provider")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| StorageProvider::from_ddb_str(s))
            .ok_or_else(|| anyhow::anyhow!("missing or invalid field: provider"))?;

        let provider_file_id: String = item
            .get("provider_file_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing required field: provider_file_id"))?;

        let size_bytes: u64 = item
            .get("size_bytes")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("missing required field: size_bytes"))?;

        let delivered_at = item
            .get("delivered_at")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .ok_or_else(|| anyhow::anyhow!("missing required field: delivered_at"))?;

        let expires_at = item
            .get("expires_at")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .ok_or_else(|| anyhow::anyhow!("missing required field: expires_at"))?;

        let record = DeliveryRecord {
            delivery_id,
            doc_id,
            sender_id,
            provider,
            provider_file_id,
            size_bytes,
            delivered_at,
            expires_at,
            burn_after_read: item
                .get("burn_after_read")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false),
        };

        Ok(Some(record))
    }
}
