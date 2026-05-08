// DeliveryEnvelope: wire format per ADR 0002 §D3.
//
// K_content is a fresh 32-byte random key per delivery.
// Body AEAD uses K_content. Per-recipient slots wrap K_content via the hybrid KEM.
// AAD on the body binds: version, suite_id, content_id, sender info, recipient set.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{bail, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::kem::{decapsulate, encapsulate, RecipientPublicKey, RecipientSecretKey, ENCAP_LEN};

pub const SUITE_MLKEM768_P256_HKDFSHA384_AES256GCM: u16 = 0x0040;
pub const WRAPPED_KEY_LEN: usize = 32 + 16; // key + AEAD tag

/// Per-recipient slot in the envelope header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipientSlot {
    pub recipient_id: Uuid,
    pub recipient_key_id: Uuid,
    /// HPKE encapsulation output: ML-KEM-768 ct (1088 B) + P-256 eph pk (33 B).
    pub encap: Vec<u8>,
    /// AES-256-GCM(K_content || nonce) wrapped for this recipient's KEM-derived key.
    pub wrapped_key: Vec<u8>,
}

/// Envelope header — stored in DynamoDB. Body stored separately in S3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeHeader {
    pub version: u8,
    pub suite_id: u16,
    pub content_id: Uuid,
    pub sender_id: Uuid,
    pub sender_key_id: Uuid,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub burn_after_read: bool,
    pub recipients: Vec<RecipientSlot>,
}

/// Body ciphertext — stored in S3 at key = content_id.
#[derive(Debug)]
pub struct EnvelopeBody {
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

/// Content encryption key — in memory only, never persisted.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 32]);

impl EnvelopeHeader {
    /// Canonical byte representation used as AAD on the body AEAD.
    /// Binds suite, sender, recipients, and expiry to the body ciphertext.
    pub fn aad(&self) -> Vec<u8> {
        let mut h = Sha384::new();
        h.update([self.version]);
        h.update(self.suite_id.to_be_bytes());
        h.update(self.content_id.as_bytes());
        h.update(self.sender_id.as_bytes());
        h.update(self.sender_key_id.as_bytes());
        h.update(self.created_at_ms.to_be_bytes());
        h.update(self.expires_at_ms.to_be_bytes());
        h.update([self.burn_after_read as u8]);
        for slot in &self.recipients {
            h.update(slot.recipient_id.as_bytes());
            h.update(slot.recipient_key_id.as_bytes());
            h.update(&slot.encap);
        }
        h.finalize().to_vec()
    }
}

/// Seal plaintext into an envelope for a set of recipients.
///
/// Returns the header (DynamoDB) and encrypted body (S3).
pub fn seal(
    plaintext: &[u8],
    sender_id: Uuid,
    sender_key_id: Uuid,
    expires_at_ms: u64,
    burn_after_read: bool,
    recipients: &[(Uuid, Uuid, &RecipientPublicKey)], // (recipient_id, key_id, pk)
) -> Result<(EnvelopeHeader, EnvelopeBody)> {
    if recipients.is_empty() {
        bail!("at least one recipient required");
    }

    // Generate fresh K_content
    let mut k_content_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut k_content_bytes);
    let k_content = ContentKey(k_content_bytes);

    let content_id = Uuid::new_v4();
    let created_at_ms = chrono::Utc::now().timestamp_millis() as u64;

    // Build recipient slots
    let mut slots = Vec::with_capacity(recipients.len());
    for (recipient_id, key_id, pk) in recipients {
        let (encap_bytes, ss) = encapsulate(&pk.kem, &pk.ec)?;
        let wrapped_key = wrap_key(&k_content.0, &ss.0)?;
        slots.push(RecipientSlot {
            recipient_id: *recipient_id,
            recipient_key_id: *key_id,
            encap: encap_bytes.to_vec(),
            wrapped_key,
        });
    }

    let header = EnvelopeHeader {
        version: 1,
        suite_id: SUITE_MLKEM768_P256_HKDFSHA384_AES256GCM,
        content_id,
        sender_id,
        sender_key_id,
        created_at_ms,
        expires_at_ms,
        burn_after_read,
        recipients: slots,
    };

    // Encrypt body with K_content; AAD = canonical header hash
    let body = encrypt_body(plaintext, &k_content, &header.aad())?;

    Ok((header, body))
}

/// Open an envelope for a specific recipient.
pub fn open(
    header: &EnvelopeHeader,
    body: &EnvelopeBody,
    recipient_id: Uuid,
    sk: &RecipientSecretKey,
    pk: &RecipientPublicKey,
) -> Result<Vec<u8>> {
    let slot = header
        .recipients
        .iter()
        .find(|s| s.recipient_id == recipient_id)
        .ok_or_else(|| anyhow::anyhow!("recipient not in envelope"))?;

    if slot.encap.len() != ENCAP_LEN {
        bail!("invalid encap length");
    }
    let encap: [u8; ENCAP_LEN] = slot.encap.as_slice().try_into()?;

    let ss = decapsulate(&encap, &sk.kem, &sk.ec, &pk.ec)?;
    let k_content_bytes = unwrap_key(&slot.wrapped_key, &ss.0)?;
    let k_content = ContentKey(k_content_bytes);

    decrypt_body(body, &k_content, &header.aad())
}

/// Wrap K_content under the KEM-derived shared secret using AES-256-GCM.
fn wrap_key(k_content: &[u8; 32], ss: &[u8; 48]) -> Result<Vec<u8>> {
    // Use first 32 bytes of the 48-byte shared secret as the wrapping key.
    // The remaining 16 are implicit entropy; the full ss is KEM-derived.
    let wrap_key = Key::<Aes256Gcm>::from_slice(&ss[..32]);
    let cipher = Aes256Gcm::new(wrap_key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let mut ct = cipher
        .encrypt(&nonce, k_content.as_ref())
        .map_err(|_| anyhow::anyhow!("key wrap encryption failed"))?;
    // Prepend nonce so the slot is self-contained: [12B nonce][32B key ct][16B tag] = 60B
    let mut out = nonce.to_vec();
    out.append(&mut ct);
    Ok(out)
}

fn unwrap_key(wrapped: &[u8], ss: &[u8; 48]) -> Result<[u8; 32]> {
    if wrapped.len() < 12 {
        bail!("wrapped key too short");
    }
    let (nonce_bytes, ct) = wrapped.split_at(12);
    let wrap_key = Key::<Aes256Gcm>::from_slice(&ss[..32]);
    let cipher = Aes256Gcm::new(wrap_key);
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ct)
        .map_err(|_| anyhow::anyhow!("key unwrap failed — wrong key or tampered ciphertext"))?;
    plaintext.as_slice().try_into().map_err(|_| anyhow::anyhow!("unwrapped key wrong length"))
}

fn encrypt_body(
    plaintext: &[u8],
    k_content: &ContentKey,
    aad: &[u8],
) -> Result<EnvelopeBody> {
    let key = Key::<Aes256Gcm>::from_slice(&k_content.0);
    let cipher = Aes256Gcm::new(key);
    let nonce_arr = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce_arr, aes_gcm::aead::Payload { msg: plaintext, aad })
        .map_err(|_| anyhow::anyhow!("body encryption failed"))?;
    Ok(EnvelopeBody {
        nonce: nonce_arr.into(),
        ciphertext,
    })
}

fn decrypt_body(
    body: &EnvelopeBody,
    k_content: &ContentKey,
    aad: &[u8],
) -> Result<Vec<u8>> {
    let key = Key::<Aes256Gcm>::from_slice(&k_content.0);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&body.nonce);
    cipher
        .decrypt(nonce, aes_gcm::aead::Payload { msg: &body.ciphertext, aad })
        .map_err(|_| anyhow::anyhow!("body decryption failed — wrong key, tampered ciphertext, or wrong AAD"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kem::generate_keypair;

    #[test]
    fn seal_open_roundtrip() {
        let (pk, sk) = generate_keypair();
        let recipient_id = Uuid::new_v4();
        let key_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let plaintext = b"hello aegis encrypted delivery";
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal(
            plaintext,
            sender_id,
            sender_key_id,
            expires,
            false,
            &[(recipient_id, key_id, &pk)],
        )
        .unwrap();

        let decrypted = open(&header, &body, recipient_id, &sk, &pk).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (pk1, sk1) = generate_keypair();
        let (pk2, _sk2) = generate_keypair();
        let rid1 = Uuid::new_v4();
        let rid2 = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal(
            b"secret",
            sender_id,
            sender_key_id,
            expires,
            false,
            &[(rid1, Uuid::new_v4(), &pk1)],
        )
        .unwrap();

        // rid2 is not in the envelope
        let result = open(&header, &body, rid2, &sk1, &pk2);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_body_fails_aad() {
        let (pk, sk) = generate_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, mut body) = seal(
            b"tamper me",
            sender_id,
            sender_key_id,
            expires,
            false,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        body.ciphertext[0] ^= 0xff; // flip a byte
        let result = open(&header, &body, recipient_id, &sk, &pk);
        assert!(result.is_err());
    }
}
