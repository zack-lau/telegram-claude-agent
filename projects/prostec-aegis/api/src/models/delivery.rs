use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// DynamoDB row shape for a delivery (envelope header stored here; body in S3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub delivery_id: Uuid,
    pub content_id: Uuid,
    pub sender_id: Uuid,
    pub sender_key_id: Uuid,
    pub suite_id: u16,
    /// Serialized EnvelopeHeader (JSON) — includes recipient slots.
    pub envelope_header: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub burn_after_read: bool,
    pub decrypted_at: Option<DateTime<Utc>>,
    pub decrypted_by_token_id: Option<Uuid>,
}

/// Request body for creating a delivery (sent by AI agent).
#[derive(Debug, Deserialize)]
pub struct CreateDeliveryRequest {
    pub recipient_ids: Vec<Uuid>,
    /// Base64-encoded plaintext payload (max 1 MB before encryption).
    pub payload_b64: String,
    pub expires_in_seconds: Option<u64>,
    pub burn_after_read: Option<bool>,
    pub metadata: Option<serde_json::Value>,
}

/// Response for a created delivery.
#[derive(Debug, Serialize)]
pub struct CreateDeliveryResponse {
    pub delivery_id: Uuid,
    pub content_id: Uuid,
    pub recipient_count: usize,
    pub expires_at: DateTime<Utc>,
}

/// Response for fetching a delivery (recipient side).
#[derive(Debug, Serialize)]
pub struct DeliveryResponse {
    pub delivery_id: Uuid,
    pub sender_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub burn_after_read: bool,
    /// Serialized envelope header for client-side decryption.
    pub envelope_header: String,
    /// Base64-encoded body ciphertext from S3.
    pub body_ciphertext_b64: String,
    pub body_nonce_b64: String,
}
