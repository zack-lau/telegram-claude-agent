// OPAQUE key registration/login flows per ADR 0003 §D5.
//
// The server stores: OPAQUE server state (password verifier), enc_sk (AES-256-KWP blob).
// The server NEVER sees: the password, the export_key, k_wrap, or sk_id in plaintext.
//
// Wire format per ADR 0003 §D5:
//   k_wrap  = OPAQUE export_key (48 bytes from RFC 9807 §4.1.2), client takes [..32]
//   enc_sk  = AES-256-KWP(k_wrap[..32], private_key_bytes)   // deterministic, no nonce
//
// AES-256-KWP is preferred over AES-256-GCM for this blob because (a) it is deterministic
// (no nonce reuse risk if k_wrap is ever reused across versions); (b) it is FIPS 140-3
// approved per SP 800-38F §6.3; and (c) it removes the GCM nonce field from the wire format.
//
// Full OPAQUE-ke Rust crate integration is stubbed here with clear TODOs
// until the opaque-ke crate API is pinned for RFC 9807.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server-side state persisted between registration start and finish.
/// Stored in DynamoDB `key_directory` with TTL = 10 minutes.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpaqueRegistrationState {
    pub recipient_id: Uuid,
    /// OPAQUE server registration state (opaque-ke::ServerRegistration blind).
    pub server_state: Vec<u8>,
    pub created_at_ms: u64,
}

/// The encrypted private key blob stored server-side.
/// Decryptable only by the client with the OPAQUE-derived export_key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedKeyBlob {
    /// AES-256-KWP wrapping of (sk_id || pk_id), KEK = OPAQUE export_key[..32] (ADR 0003 §D5).
    pub enc_sk: Vec<u8>,
    /// AES-256-KWP wrapping of (sk_id || pk_id), KEK = HKDF(recovery_code, "aegis-v1-recovery-wrap")[..32].
    /// Null until user completes recovery code ceremony.
    pub enc_sk_recovery: Option<Vec<u8>>,
    pub key_version: u8,
    pub created_at_ms: u64,
}

/// Recipient registration flow — step 1 (client blind → server response).
/// Returns: server response bytes to send back to client + state to persist.
///
/// TODO: integrate opaque-ke::ServerRegistration when RFC 9807 support stabilizes.
/// The shape here matches the expected opaque-ke 2.x API.
pub fn registration_start(
    recipient_id: Uuid,
    blinded_message: &[u8],
) -> Result<(Vec<u8>, OpaqueRegistrationState)> {
    // TODO: replace with opaque_ke::ServerRegistration::start(...)
    // let (server_registration_start_result, server_registration) =
    //     opaque_ke::ServerRegistration::<AegisOpaqueConfig>::start(
    //         &server_setup,
    //         opaque_ke::RegistrationRequest::deserialize(blinded_message)?,
    //         recipient_id.as_bytes(),
    //     )?;
    // let response = server_registration_start_result.serialize().to_vec();
    // let state = server_registration.serialize().to_vec();

    let _ = (recipient_id, blinded_message); // suppress unused warnings
    Err(anyhow::anyhow!("opaque-ke integration pending — see TODO in opaque.rs"))
}

/// Recipient registration flow — step 2 (client finalize → server stores).
/// Returns: EncryptedKeyBlob to persist in `key_directory`.
///
/// TODO: integrate opaque-ke::ServerRegistration::finish(...)
pub fn registration_finish(
    state: &OpaqueRegistrationState,
    registration_upload: &[u8],
    enc_sk: Vec<u8>,
) -> Result<EncryptedKeyBlob> {
    let _ = (state, registration_upload, enc_sk);
    Err(anyhow::anyhow!("opaque-ke integration pending — see TODO in opaque.rs"))
}

/// Login flow — step 1 (client blind → server response).
///
/// TODO: integrate opaque-ke::ServerLogin::start(...)
pub fn login_start(
    password_file: &[u8],
    client_message: &[u8],
    recipient_id: Uuid,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let _ = (password_file, client_message, recipient_id);
    Err(anyhow::anyhow!("opaque-ke integration pending — see TODO in opaque.rs"))
}

/// Login flow — step 2 (client finalize → server confirms + returns enc_sk).
///
/// On success, server returns enc_sk for client to decrypt with export_key.
///
/// TODO: integrate opaque-ke::ServerLogin::finish(...)
pub fn login_finish(
    server_login_state: &[u8],
    client_message: &[u8],
) -> Result<()> {
    let _ = (server_login_state, client_message);
    Err(anyhow::anyhow!("opaque-ke integration pending — see TODO in opaque.rs"))
}
