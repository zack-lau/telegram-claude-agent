use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Cloud storage providers supported by Aegis (MVP: Google Drive + OneDrive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageProvider {
    #[serde(rename = "google_drive")]
    GoogleDrive,
    /// Serialized as "onedrive" (no underscore) — must match oauth-tokens table,
    /// oauth_refresh_worker, and /envelopes/fetch provider strings.
    #[serde(rename = "onedrive")]
    OneDrive,
}

impl StorageProvider {
    pub fn as_ddb_str(self) -> &'static str {
        match self {
            StorageProvider::GoogleDrive => "google_drive",
            StorageProvider::OneDrive => "onedrive",
        }
    }

    pub fn from_ddb_str(s: &str) -> Option<Self> {
        match s {
            "google_drive" => Some(StorageProvider::GoogleDrive),
            "onedrive" => Some(StorageProvider::OneDrive),
            _ => None,
        }
    }
}

/// Validates a provider-assigned file ID. Shared by streaming::init and envelopes::fetch_envelope.
///
/// Allowlist rationale:
/// - Google Drive IDs: [A-Za-z0-9_-], always ≥28 chars
/// - OneDrive item IDs: [A-Za-z0-9_-!], may contain `!` in driveItem IDs
/// - Min length 8: rejects path traversal tokens like ".." or "." with no real ID semantics
pub fn provider_file_id_is_valid(id: &str) -> bool {
    if id.len() < 8 || id.len() > 512 { return false; }
    if id.contains("..") { return false; }
    if id.contains('\0') || id.contains('\r') || id.contains('\n') { return false; }
    id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '~'))
}

/// DynamoDB row shape for a delivery (metadata only; encrypted body in recipient's cloud storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub delivery_id: Uuid,
    /// Envelope content ID from the sealed header — distinct from delivery_id (the DDB row key).
    /// Used for recipient-side dedup and the doc-id-index GSI.
    pub doc_id: Uuid,
    pub sender_id: Uuid,
    /// Cloud storage provider where the agent deposited the encrypted envelope.
    pub provider: StorageProvider,
    /// Provider-assigned file ID for the encrypted envelope (stable across renames/moves).
    pub provider_file_id: String,
    /// Size of the encrypted envelope in bytes.
    pub size_bytes: u64,
    /// Delivery timestamp — DDB attribute "delivered_at", range key on sender-index GSI.
    pub delivered_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub burn_after_read: bool,
}

/// Request body for creating a delivery record (sent by AI agent).
/// The agent uploads the encrypted envelope to the recipient's cloud storage directly;
/// Aegis only records that a delivery happened.
#[derive(Debug, Deserialize)]
pub struct CreateDeliveryRequest {
    pub recipient_ids: Vec<Uuid>,
    /// Envelope content ID from the sealed header, supplied by the agent.
    pub doc_id: Uuid,
    /// Cloud storage provider where the agent deposited the encrypted envelope.
    pub provider: StorageProvider,
    /// Provider-assigned file ID for the encrypted envelope.
    pub provider_file_id: String,
    /// Size of the encrypted envelope in bytes.
    pub size_bytes: u64,
    pub expires_in_seconds: Option<u64>,
    pub burn_after_read: Option<bool>,
    pub metadata: Option<serde_json::Value>,
}

/// Response for a created delivery.
#[derive(Debug, Serialize)]
pub struct CreateDeliveryResponse {
    pub delivery_id: Uuid,
    pub recipient_count: usize,
    pub expires_at: DateTime<Utc>,
}

/// Response for fetching a delivery — metadata only; no body content.
/// The encrypted envelope lives in the recipient's cloud storage at provider_file_id.
#[derive(Debug, Serialize)]
pub struct DeliveryResponse {
    /// Per-recipient DDB row identifier.
    pub delivery_id: Uuid,
    /// Envelope content ID — use this for dedup, not delivery_id.
    pub doc_id: Uuid,
    pub sender_id: Uuid,
    pub recipient_id: Uuid,
    /// Cloud storage provider where the encrypted envelope can be retrieved.
    pub provider: StorageProvider,
    /// Provider-assigned file ID for the encrypted envelope.
    pub provider_file_id: String,
    pub size_bytes: u64,
    pub delivered_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_provider_ddb_round_trip() {
        for p in [StorageProvider::GoogleDrive, StorageProvider::OneDrive] {
            assert_eq!(StorageProvider::from_ddb_str(p.as_ddb_str()), Some(p));
        }
    }

    #[test]
    fn storage_provider_serde_round_trip() {
        let cases = [
            (StorageProvider::GoogleDrive, r#""google_drive""#),
            (StorageProvider::OneDrive, r#""onedrive""#),
        ];
        for (provider, expected_json) in cases {
            let serialized = serde_json::to_string(&provider).unwrap();
            assert_eq!(serialized, expected_json);
            let deserialized: StorageProvider = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, provider);
        }
    }

    #[test]
    fn provider_file_id_valid_cases() {
        assert!(provider_file_id_is_valid("1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgVE2upms")); // Google Drive
        assert!(provider_file_id_is_valid("01ABCDEFGHIJKLMNOPQRST")); // OneDrive-style
        assert!(provider_file_id_is_valid("abcdefgh")); // min length 8
    }

    #[test]
    fn provider_file_id_invalid_cases() {
        assert!(!provider_file_id_is_valid("")); // empty
        assert!(!provider_file_id_is_valid("short")); // < 8 chars
        assert!(!provider_file_id_is_valid("../../etc/passwd12")); // path traversal
        assert!(!provider_file_id_is_valid("abc\0def12")); // null byte
        assert!(!provider_file_id_is_valid("abc\ndef12")); // newline
        assert!(!provider_file_id_is_valid(&"a".repeat(513))); // too long
        assert!(!provider_file_id_is_valid("http://evil12")); // colon (scheme)
    }
}
