use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// DynamoDB row in `oauth-tokens` table.
/// hash=recipient_id, range=token_id per ADR 0001.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub recipient_id: Uuid,
    pub token_id: Uuid,
    pub token_value_hash: String,
    pub token_type: String,
    pub token_family_id: Uuid,
    pub version: u64,
    pub auth_provider: String,
    pub expires_at: i64,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub device_hint: String,
    pub ip_at_creation: String,
    pub session_epoch: u64,
    pub trusted: bool,
}

/// Active session visible to the recipient (GET /me/sessions).
#[derive(Debug, Serialize)]
pub struct SessionView {
    /// Partial token_id for UI display (first 8 chars).
    pub token_id_prefix: String,
    pub device_hint: String,
    pub ip_at_creation: String,
    pub last_used_at: DateTime<Utc>,
    pub auth_provider: String,
    pub created_at: DateTime<Utc>,
    pub trusted: bool,
}
