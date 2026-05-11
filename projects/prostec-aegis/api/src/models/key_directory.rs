use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::key_bundle::KeyBundleSigned;

/// DynamoDB row in `key-directory` table.
///
/// The signed bundle is stored as a JSON blob (`signed_bundle_json`) — the AIK signs
/// the canonical_bundle_bytes() of the inner KeyBundle, NOT the JSON, so the JSON
/// is just a transport encoding the directory uses for storage and retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyDirectoryRecord {
    pub recipient_id: Uuid,
    /// Bundle version, denormalized for quick rollback checks without parsing the JSON.
    pub bundle_version: u64,
    /// Bundle expiry, denormalized for fast TTL checks.
    pub bundle_expiry: DateTime<Utc>,
    /// AIK fingerprint (truncated SHA-256 of account_identity_pubkey, hex). Indexed for lookup.
    pub aik_fingerprint_hex: String,
    /// JSON-serialized `KeyBundleSigned` (AIK-signed bundle per ADR 0003 §D7).
    pub signed_bundle_json: String,
    /// OPAQUE encrypted private key blob — AES-256-KWP(k_wrap[..32], private_key_bytes) per ADR 0003 §D5.
    pub enc_sk_b64: String,
    /// OPAQUE encrypted private key blob for recovery code path.
    pub enc_sk_recovery_b64: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl KeyDirectoryRecord {
    pub fn parse_signed_bundle(&self) -> Result<KeyBundleSigned, serde_json::Error> {
        serde_json::from_str(&self.signed_bundle_json)
    }
}

/// Request to register or update a recipient's key bundle.
/// The client constructs the full AIK-signed bundle locally and posts it as JSON.
#[derive(Debug, Deserialize)]
pub struct RegisterKeyBundleRequest {
    /// Complete AIK-signed key bundle (`KeyBundleSigned`) as JSON.
    pub signed_bundle: KeyBundleSigned,
    /// Encrypted private key blob from OPAQUE registration (AES-256-KWP wrapped).
    pub enc_sk_b64: String,
    pub enc_sk_recovery_b64: Option<String>,
}

/// Response: signed key bundle for senders to encapsulate to.
/// Senders verify `signed_bundle` against the pinned AIK fingerprint.
#[derive(Debug, Serialize)]
pub struct KeyBundleResponse {
    pub recipient_id: Uuid,
    /// AIK fingerprint (hex). Sender pins this out-of-band; bundle verification
    /// requires the embedded AIK pubkey to hash to this value.
    pub aik_fingerprint_hex: String,
    pub bundle_version: u64,
    pub bundle_expiry: DateTime<Utc>,
    /// Full signed bundle for client-side verification. Self-contained — no further
    /// fetch needed to verify.
    pub signed_bundle: KeyBundleSigned,
}
