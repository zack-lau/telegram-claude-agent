// DeliveryEnvelope: wire format per ADR 0003 §D7.
//
// K_content is a fresh 32-byte random key per delivery.
// Body AEAD uses K_content. Per-recipient slots wrap K_content via the hybrid KEM.
// AAD on the body binds: version, suite_id, content_id, sender info, recipient set.

use aes::Aes256;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use aes_kw::Kek;
use anyhow::{bail, Result};
use hkdf::Hkdf;
use p256::ecdsa::{
    Signature as EcdsaSignature, SigningKey as EcdsaSigningKey, VerifyingKey as EcdsaVerifyingKey,
};
use p256::ecdsa::signature::{Signer, Verifier};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::kem::{decapsulate, encapsulate, RecipientPublicKey, RecipientSecretKey, ENCAP_LEN};

pub const SUITE_MLKEM768_P256_HKDFSHA384_AES256GCM: u16 = 0x0040;
/// [8B ICV][32B K_content wrapped] = 40B (AES-256-KWP, RFC 5649 / NIST SP 800-38F)
pub const WRAPPED_KEY_LEN: usize = 8 + 32;

/// Distinct HKDF label for the key-wrapping key derivation.
/// Separates this usage from the KEM shared-secret domain.
const WRAP_KEY_LABEL: &[u8] = b"aegis-v1 envelope key-wrap";

/// Per-recipient slot in the envelope header.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipientSlot {
    pub recipient_id: Uuid,
    pub recipient_key_id: Uuid,
    /// HPKE encapsulation output: ML-KEM-768 ct (1088 B) + P-256 eph pk (33 B) = 1121 B.
    pub encap: Vec<u8>,
    /// AES-256-KWP wrapped K_content: [8B ICV][32B wrapped] = 40 B (RFC 5649).
    pub wrapped_key: Vec<u8>,
}

impl RecipientSlot {
    pub fn validate(&self) -> Result<()> {
        if self.encap.len() != ENCAP_LEN {
            bail!("encap must be {} bytes, got {}", ENCAP_LEN, self.encap.len());
        }
        if self.wrapped_key.len() != WRAPPED_KEY_LEN {
            bail!("wrapped_key must be {} bytes, got {}", WRAPPED_KEY_LEN, self.wrapped_key.len());
        }
        Ok(())
    }
}

/// Envelope header — stored in DynamoDB. Body stored separately in S3.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeHeader {
    pub version: u8,
    pub suite_id: u16,
    pub content_id: Uuid,
    pub sender_id: Uuid,
    pub sender_key_id: Uuid,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub burn_after_read: bool,
    /// Cap at 64 recipients to bound deserialization work.
    pub recipients: Vec<RecipientSlot>,
    /// ECDSA-P256 signature over aad() bytes — 64 bytes (r||s). Sender auth,
    /// KCI-resistant (forgery requires sender private key, not recipient key).
    pub sender_signature: Vec<u8>,
}

impl EnvelopeHeader {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported envelope version {}", self.version);
        }
        if self.recipients.is_empty() || self.recipients.len() > 64 {
            bail!("recipients must be 1..=64, got {}", self.recipients.len());
        }
        for slot in &self.recipients {
            slot.validate()?;
        }
        if self.sender_signature.len() != 64 {
            bail!("sender_signature must be 64 bytes, got {}", self.sender_signature.len());
        }
        Ok(())
    }
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

/// ECDSA-P256 signing key for sender authentication (KCI-resistant).
pub struct SenderSigningKey(pub EcdsaSigningKey);

/// ECDSA-P256 verifying key for sender authentication.
pub struct SenderVerifyingKey(pub EcdsaVerifyingKey);

/// Generate a fresh P-256 ECDSA sender keypair.
pub fn generate_sender_keypair() -> (SenderVerifyingKey, SenderSigningKey) {
    let p256_sk = p256::SecretKey::random(&mut OsRng);
    let sk = EcdsaSigningKey::from(&p256_sk);
    let vk = sk.verifying_key().clone();
    (SenderVerifyingKey(vk), SenderSigningKey(sk))
}

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
    sender_sk: &SenderSigningKey,
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

    let mut header = EnvelopeHeader {
        version: 1,
        suite_id: SUITE_MLKEM768_P256_HKDFSHA384_AES256GCM,
        content_id,
        sender_id,
        sender_key_id,
        created_at_ms,
        expires_at_ms,
        burn_after_read,
        recipients: slots,
        sender_signature: Vec::new(), // filled below
    };

    // AAD binds all header fields except sender_signature (can't sign yourself).
    let aad = header.aad();

    // Sign over AAD with sender's ECDSA-P256 key — KCI-resistant sender auth.
    let sig: EcdsaSignature = sender_sk.0.sign(&aad);
    header.sender_signature = sig.to_bytes().to_vec();

    let body = encrypt_body(plaintext, &k_content, &aad)?;

    Ok((header, body))
}

/// Open an envelope for a specific recipient.
pub fn open(
    header: &EnvelopeHeader,
    body: &EnvelopeBody,
    recipient_id: Uuid,
    sk: &RecipientSecretKey,
    pk: &RecipientPublicKey,
    sender_vk: &SenderVerifyingKey,
) -> Result<Vec<u8>> {
    // Verify sender signature before any decryption work.
    let aad = header.aad();
    let sig = EcdsaSignature::from_slice(&header.sender_signature)
        .map_err(|_| anyhow::anyhow!("invalid sender signature format"))?;
    sender_vk.0.verify(&aad, &sig)
        .map_err(|_| anyhow::anyhow!("sender signature verification failed"))?;

    let slot = header
        .recipients
        .iter()
        .find(|s| s.recipient_id == recipient_id)
        .ok_or_else(|| anyhow::anyhow!("recipient not in envelope"))?;

    slot.validate()?;
    let encap: [u8; ENCAP_LEN] = slot.encap.as_slice().try_into()?;

    let ss = decapsulate(&encap, &sk.kem, &sk.ec, &pk.ec)?;
    let k_content_bytes = unwrap_key(&slot.wrapped_key, &ss.0)?;
    let k_content = ContentKey(k_content_bytes);

    decrypt_body(body, &k_content, &aad)
}

/// Derive a 32-byte wrapping key from the KEM shared secret with domain separation.
/// Uses HKDF-SHA384 with a distinct label so this key material is independent of
/// any other usage of the shared secret.
fn derive_wrap_key(ss: &[u8; 48]) -> [u8; 32] {
    let hk = Hkdf::<Sha384>::new(None, ss);
    let mut okm = [0u8; 32];
    hk.expand(WRAP_KEY_LABEL, &mut okm).expect("hkdf expand is infallible for 32-byte output");
    okm
}

/// Wrap K_content using AES-256-KWP (RFC 5649, NIST SP 800-38F §6.3).
/// Wire format: [8B ICV][32B K_content wrapped] = 40 B. Deterministic — no nonce.
fn wrap_key(k_content: &[u8; 32], ss: &[u8; 48]) -> Result<Vec<u8>> {
    let mut wrap_key_bytes = derive_wrap_key(ss);
    let kek = Kek::<Aes256>::try_from(wrap_key_bytes.as_ref())
        .expect("32-byte key is valid for AES-256-KWP");
    let wrapped = kek.wrap_with_padding_vec(k_content.as_ref())
        .map_err(|_| anyhow::anyhow!("key wrap failed"))?;
    wrap_key_bytes.zeroize();
    debug_assert_eq!(wrapped.len(), WRAPPED_KEY_LEN);
    Ok(wrapped)
}

fn unwrap_key(wrapped: &[u8], ss: &[u8; 48]) -> Result<[u8; 32]> {
    if wrapped.len() != WRAPPED_KEY_LEN {
        bail!("wrapped_key must be {} bytes, got {}", WRAPPED_KEY_LEN, wrapped.len());
    }
    let mut wrap_key_bytes = derive_wrap_key(ss);
    let kek = Kek::<Aes256>::try_from(wrap_key_bytes.as_ref())
        .expect("32-byte key is valid for AES-256-KWP");
    let plaintext = kek.unwrap_with_padding_vec(wrapped)
        .map_err(|_| anyhow::anyhow!("key unwrap failed — wrong key or tampered data"))?;
    wrap_key_bytes.zeroize();
    plaintext.as_slice().try_into()
        .map_err(|_| anyhow::anyhow!("unwrapped key wrong length"))
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
        let (sender_vk, sender_sk) = generate_sender_keypair();
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
            &sender_sk,
            expires,
            false,
            &[(recipient_id, key_id, &pk)],
        )
        .unwrap();

        assert_eq!(header.sender_signature.len(), 64);
        let decrypted = open(&header, &body, recipient_id, &sk, &pk, &sender_vk).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (pk1, sk1) = generate_keypair();
        let (pk2, _sk2) = generate_keypair();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let rid1 = Uuid::new_v4();
        let rid2 = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal(
            b"secret",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            false,
            &[(rid1, Uuid::new_v4(), &pk1)],
        )
        .unwrap();

        let result = open(&header, &body, rid2, &sk1, &pk2, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_body_fails_aad() {
        let (pk, sk) = generate_keypair();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, mut body) = seal(
            b"tamper me",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            false,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        body.ciphertext[0] ^= 0xff;
        let result = open(&header, &body, recipient_id, &sk, &pk, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_signature_rejected() {
        let (pk, sk) = generate_keypair();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (mut header, body) = seal(
            b"sign me",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            false,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        header.sender_signature[0] ^= 0xff;
        let result = open(&header, &body, recipient_id, &sk, &pk, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_sender_key_rejected() {
        let (pk, sk) = generate_keypair();
        let (_sender_vk, sender_sk) = generate_sender_keypair();
        let (wrong_vk, _) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal(
            b"who sent this?",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            false,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        let result = open(&header, &body, recipient_id, &sk, &pk, &wrong_vk);
        assert!(result.is_err());
    }
}
