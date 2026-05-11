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
        // Strongly consistent — a freshly rotated bundle MUST be visible to senders
        // immediately, otherwise an attacker could exploit the eventual-consistency
        // window to encapsulate to a revoked AIK (Qwen Round 6 db HIGH).
        let result = self.ddb
            .get_item()
            .consistent_read(true)
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb get error: {}", e))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };

        let bundle_version = item
            .get("bundle_version")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("missing required field: bundle_version"))?;

        let bundle_expiry = item
            .get("bundle_expiry")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .ok_or_else(|| anyhow::anyhow!("missing required field: bundle_expiry"))?;

        let aik_fingerprint_hex = item
            .get("aik_fingerprint_hex")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing required field: aik_fingerprint_hex"))?;

        let signed_bundle_json = item
            .get("signed_bundle_json")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing required field: signed_bundle_json"))?;

        let enc_sk_b64 = item
            .get("enc_sk")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing required field: enc_sk"))?;

        let created_at = item
            .get("created_at")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .ok_or_else(|| anyhow::anyhow!("missing required field: created_at"))?;

        Ok(Some(KeyDirectoryRecord {
            recipient_id,
            bundle_version,
            bundle_expiry,
            aik_fingerprint_hex,
            signed_bundle_json,
            enc_sk_b64,
            enc_sk_recovery_b64: item.get("enc_sk_recovery").and_then(|v| v.as_s().ok()).cloned(),
            created_at,
        }))
    }

    /// Insert or rotate a key bundle. Enforces TWO things via conditional write:
    ///   1. Monotonic bundle_version (rollback blocked at storage layer).
    ///   2. AIK CONTINUITY — once an AIK has been registered for a recipient, the same
    ///      AIK fingerprint MUST appear on every subsequent bundle. A new AIK requires
    ///      explicit AIK rotation (separate ceremony, not implemented for MVP).
    ///
    /// Without (2), an attacker holding a Cognito session could publish their own AIK
    /// at version+1 and silently take over the account directory. (Codex Round 2 finding.)
    ///
    /// Returns Conflict-class error if either condition fails.
    pub async fn put(&self, record: &KeyDirectoryRecord) -> Result<()> {
        let mut builder = self.ddb
            .put_item()
            .table_name(&self.table)
            .item("recipient_id", AttributeValue::S(record.recipient_id.to_string()))
            .item("bundle_version", AttributeValue::N(record.bundle_version.to_string()))
            .item("bundle_expiry", AttributeValue::S(record.bundle_expiry.to_rfc3339()))
            .item("aik_fingerprint_hex", AttributeValue::S(record.aik_fingerprint_hex.clone()))
            .item("signed_bundle_json", AttributeValue::S(record.signed_bundle_json.clone()))
            .item("enc_sk", AttributeValue::S(record.enc_sk_b64.clone()))
            .item("created_at", AttributeValue::S(record.created_at.to_rfc3339()))
            // Either: no row exists yet (first registration; ANY AIK accepted)
            // Or:     row exists AND bundle_version is strictly increasing AND aik_fingerprint matches
            .condition_expression(
                "attribute_not_exists(recipient_id) \
                 OR (bundle_version < :new_v AND aik_fingerprint_hex = :aik_fp)",
            )
            .expression_attribute_values(":new_v", AttributeValue::N(record.bundle_version.to_string()))
            .expression_attribute_values(":aik_fp", AttributeValue::S(record.aik_fingerprint_hex.clone()));

        if let Some(ref recovery) = record.enc_sk_recovery_b64 {
            builder = builder.item("enc_sk_recovery", AttributeValue::S(recovery.clone()));
        }

        builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb put error (rollback, AIK mismatch, or transient): {}", e))?;
        Ok(())
    }
}
