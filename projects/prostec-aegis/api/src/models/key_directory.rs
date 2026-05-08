use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// DynamoDB row in `key-directory` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyDirectoryRecord {
    pub recipient_id: Uuid,
    /// ML-KEM-768 encapsulation key (1184 bytes), base64url.
    pub kem_pk_b64: String,
    /// P-256 public key, SEC1 compressed (33 bytes), base64url.
    pub ec_pk_b64: String,
    pub key_version: u8,
    pub expires_at: DateTime<Utc>,
    /// Ed25519 signature over canonical bundle bytes, base64url.
    pub signature_b64: String,
    /// Aegis CA key fingerprint.
    pub signer_key_id: String,
    /// OPAQUE encrypted private key blob — AES-256-GCM(k_wrap, sk_id).
    pub enc_sk_b64: String,
    /// OPAQUE encrypted private key blob for recovery code path.
    pub enc_sk_recovery_b64: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Request to register or update a recipient's key bundle.
#[derive(Debug, Deserialize)]
pub struct RegisterKeyBundleRequest {
    pub kem_pk_b64: String,
    pub ec_pk_b64: String,
    /// Encrypted private key blob from OPAQUE registration.
    pub enc_sk_b64: String,
    pub enc_sk_recovery_b64: Option<String>,
}

/// Response: signed key bundle for senders to encapsulate to.
#[derive(Debug, Serialize)]
pub struct KeyBundleResponse {
    pub recipient_id: Uuid,
    pub kem_pk_b64: String,
    pub ec_pk_b64: String,
    pub version: u8,
    pub expires_at: DateTime<Utc>,
    pub signature_b64: String,
    pub signer_key_id: String,
}
