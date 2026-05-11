// DeliveryEnvelope: wire format per ADR 0003 §D7.
//
// K_content is a fresh 32-byte random key per delivery.
// Body AEAD uses K_content. Per-recipient slots wrap K_content via the hybrid KEM.
// AAD on the body binds: version, suite_id, content_id, sender info, recipient set.
//
// FIPS note: every primitive runs through AWS-LC FIPS module:
// AES-256-GCM, AES-256-KWP, ECDSA P-256, SHA-384, HKDF-SHA384.

use anyhow::{bail, Result};
use aws_lc_rs::{
    aead::{Aad, AES_256_GCM, LessSafeKey, Nonce, UnboundKey},
    digest::{Context as DigestContext, SHA384},
    hkdf::{self, Salt, HKDF_SHA384},
    key_wrap::{AesKek, KeyWrapPadded, AES_256},
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
                ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::error::CryptoError;
use super::kem::{decapsulate, encapsulate, RecipientPublicKey, RecipientSecretKey, ENCAP_LEN};

pub const SUITE_MLKEM768_P256_HKDFSHA384_AES256GCM: u16 = 0x0040;
/// [8B ICV][32B K_content wrapped] = 40B (AES-256-KWP, RFC 5649 / NIST SP 800-38F)
pub const WRAPPED_KEY_LEN: usize = 8 + 32;

/// Distinct HKDF label for key-wrap derivation — domain-separates from KEM usage.
const WRAP_KEY_LABEL: &[u8] = b"aegis-v1 envelope key-wrap";
/// Non-empty HKDF salt per NIST SP 800-56C Rev.2 §4 — distinct from KEM salt.
const WRAP_KEY_SALT: &[u8] = b"aegis-v1 wrap-key-salt";

/// Domain prefix for sender signatures.
const SENDER_SIG_DOMAIN: &[u8] = b"aegis-v1-sender-sig";

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

/// Envelope header — stored in DynamoDB. Body stored separately in recipient cloud storage.
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
    /// ECDSA-P256 signature over
    ///   SENDER_SIG_DOMAIN || aad() || Sha384(nonce || ciphertext)
    /// 64 bytes (r||s fixed format). Sender auth, KCI-resistant.
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

/// Body ciphertext — stored in recipient cloud storage, identified by provider_file_id.
#[derive(Debug)]
pub struct EnvelopeBody {
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>, // [encrypted plaintext | 16-byte GCM tag]
}

/// Content encryption key — in memory only, never persisted.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 32]);

/// ECDSA-P256 signing key for sender authentication (KCI-resistant).
pub struct SenderSigningKey(pub EcdsaKeyPair);

/// ECDSA-P256 verifying key for sender authentication.
/// Stored as 65-byte uncompressed SEC1 — required by aws-lc-rs ECDSA verification.
pub struct SenderVerifyingKey(pub Vec<u8>);

/// Generate a fresh P-256 ECDSA sender keypair.
/// aws-lc-rs uses random nonces per FIPS 186-5 §6.4 (both deterministic and
/// random nonces are approved).
pub fn generate_sender_keypair() -> (SenderVerifyingKey, SenderSigningKey) {
    let rng = SystemRandom::new();
    let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .expect("ECDSA P-256 key generation failed");
    let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, doc.as_ref())
        .expect("generated pkcs8 is always valid");
    let vk_bytes = kp.public_key().as_ref().to_vec(); // 65-byte uncompressed SEC1
    (SenderVerifyingKey(vk_bytes), SenderSigningKey(kp))
}

impl EnvelopeHeader {
    /// Canonical byte representation used as AAD on the body AEAD.
    /// Binds suite, sender, recipients, and expiry to the body ciphertext.
    pub fn aad(&self) -> Vec<u8> {
        let mut ctx = DigestContext::new(&SHA384);
        ctx.update(&[self.version]);
        ctx.update(&self.suite_id.to_be_bytes());
        ctx.update(self.content_id.as_bytes());
        ctx.update(self.sender_id.as_bytes());
        ctx.update(self.sender_key_id.as_bytes());
        ctx.update(&self.created_at_ms.to_be_bytes());
        ctx.update(&self.expires_at_ms.to_be_bytes());
        ctx.update(&[self.burn_after_read as u8]);
        for slot in &self.recipients {
            ctx.update(slot.recipient_id.as_bytes());
            ctx.update(slot.recipient_key_id.as_bytes());
            ctx.update(&slot.encap);
            // wrapped_key included so the body AEAD tag also binds the KWP-wrapped
            // K_content slots — KWP ICV already protects each slot individually (M-2).
            ctx.update(&slot.wrapped_key);
        }
        ctx.finish().as_ref().to_vec()
    }
}

/// Construct the sender signature message: domain || header_aad || payload_hash.
/// payload_hash = SHA384(nonce || ciphertext), streaming to avoid intermediate alloc.
fn sender_sig_message(aad: &[u8], nonce: &[u8; 12], ciphertext: &[u8]) -> Vec<u8> {
    let mut ctx = DigestContext::new(&SHA384);
    ctx.update(nonce);
    ctx.update(ciphertext);
    let payload_hash = ctx.finish();
    let mut msg = Vec::with_capacity(SENDER_SIG_DOMAIN.len() + aad.len() + 48);
    msg.extend_from_slice(SENDER_SIG_DOMAIN);
    msg.extend_from_slice(aad);
    msg.extend_from_slice(payload_hash.as_ref());
    msg
}

/// Seal plaintext into an envelope for a set of recipients.
///
/// Returns the header (DynamoDB) and encrypted body (for upload to recipient cloud storage).
pub fn seal(
    plaintext: &[u8],
    sender_id: Uuid,
    sender_key_id: Uuid,
    sender_sk: &SenderSigningKey,
    sender_sk_fp: &[u8],
    expires_at_ms: u64,
    burn_after_read: bool,
    recipients: &[(Uuid, Uuid, &RecipientPublicKey, &[u8])], // (recipient_id, key_id, pk, aik_fp)
) -> Result<(EnvelopeHeader, EnvelopeBody)> {
    if recipients.is_empty() {
        bail!("at least one recipient required");
    }
    if sender_sk_fp.len() != 32 {
        bail!("sender_sk_fp must be 32 bytes (SHA-256 fingerprint), got {}", sender_sk_fp.len());
    }
    for (_, _, _, aik_fp) in recipients {
        if aik_fp.len() != 32 {
            bail!("aik_fp must be 32 bytes (SHA-256 fingerprint), got {}", aik_fp.len());
        }
    }

    let rng = SystemRandom::new();

    // Generate fresh K_content
    let mut k_content_bytes = [0u8; 32];
    rng.fill(&mut k_content_bytes)
        .map_err(|_| anyhow::anyhow!("RNG failed generating K_content"))?;
    let k_content = ContentKey(k_content_bytes);

    let content_id = Uuid::new_v4();
    let created_at_ms = chrono::Utc::now().timestamp_millis() as u64;

    let mut slots = Vec::with_capacity(recipients.len());
    for (recipient_id, key_id, pk, aik_fp) in recipients {
        let (encap_bytes, ss) = encapsulate(
            &pk.kem,
            &pk.kem_bytes,
            &pk.ec,
            sender_sk_fp,
            aik_fp,
        )?;
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
        sender_signature: Vec::new(),
    };

    let aad = header.aad();
    let body = encrypt_body(plaintext, &k_content, &aad)?;

    let sig_msg = sender_sig_message(&aad, &body.nonce, &body.ciphertext);
    let sig = sender_sk.0.sign(&rng, &sig_msg)
        .map_err(|_| anyhow::anyhow!("ECDSA signing failed"))?;
    header.sender_signature = sig.as_ref().to_vec(); // 64-byte r||s fixed format

    Ok((header, body))
}

/// Open an envelope for a specific recipient.
///
/// `expected_key_id` must match the `recipient_key_id` in the header slot — this is
/// checked before any crypto work so a stale-key error is explicit, not a crypto failure
/// that leaks timing (Q-H1).
pub fn open(
    header: &EnvelopeHeader,
    body: &EnvelopeBody,
    recipient_id: Uuid,
    expected_key_id: Uuid,
    sk: &RecipientSecretKey,
    pk: &RecipientPublicKey,
    sender_vk: &SenderVerifyingKey,
    sender_sk_fp: &[u8],
    recipient_aik_fp: &[u8],
) -> Result<Vec<u8>> {
    if sender_vk.0.len() != 65 {
        bail!("sender_vk must be 65 bytes (uncompressed SEC1 P-256), got {}", sender_vk.0.len());
    }
    if sender_sk_fp.len() != 32 {
        bail!("sender_sk_fp must be 32 bytes (SHA-256 fingerprint), got {}", sender_sk_fp.len());
    }
    if recipient_aik_fp.len() != 32 {
        bail!("recipient_aik_fp must be 32 bytes (SHA-256 fingerprint), got {}", recipient_aik_fp.len());
    }
    // Reject malformed/oversized headers BEFORE doing any crypto work
    // (Qwen Round 3 crypto HIGH on missing validate() in open path).
    header.validate().map_err(|_| CryptoError::HeaderInvalid)?;

    // Verify sender signature before any decryption work.
    let aad = header.aad();
    let sig_msg = sender_sig_message(&aad, &body.nonce, &body.ciphertext);
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &sender_vk.0)
        .verify(&sig_msg, &header.sender_signature)
        .map_err(|_| CryptoError::SigVerifyFail)?;

    let slot = header
        .recipients
        .iter()
        .find(|s| s.recipient_id == recipient_id)
        .ok_or_else(|| anyhow::anyhow!("recipient not in envelope"))?;

    // Verify caller is using the exact key version that was used to seal this slot.
    // Mismatch means a stale key was fetched — fail explicitly before any crypto (Q-H1).
    if slot.recipient_key_id != expected_key_id {
        bail!(
            "key_id mismatch: envelope slot has {}, caller provided {}",
            slot.recipient_key_id, expected_key_id
        );
    }

    slot.validate()?;
    let encap: [u8; ENCAP_LEN] = slot.encap.as_slice().try_into()?;

    let ss = decapsulate(
        &encap,
        &sk.kem,
        &sk.ec,
        &pk.ec,
        &pk.kem_bytes,
        sender_sk_fp,
        recipient_aik_fp,
    )?;
    let k_content_bytes = unwrap_key(&slot.wrapped_key, &ss.0)?;
    let k_content = ContentKey(k_content_bytes);

    decrypt_body(body, &k_content, &aad)
}

/// Derive a 32-byte wrapping key from the KEM shared secret.
/// HKDF-SHA384 with distinct label for domain separation.
fn derive_wrap_key(ss: &[u8; 48]) -> [u8; 32] {
    struct Len32;
    impl hkdf::KeyType for Len32 {
        fn len(&self) -> usize { 32 }
    }
    let salt = Salt::new(HKDF_SHA384, WRAP_KEY_SALT);
    let prk = salt.extract(ss);
    let mut okm = [0u8; 32];
    prk.expand(&[WRAP_KEY_LABEL], Len32)
        .expect("HKDF expand is infallible for 32-byte output with HKDF-SHA384")
        .fill(&mut okm)
        .expect("buffer size matches Len32");
    okm
}

/// Wrap K_content using AES-256-KWP (RFC 5649 / NIST SP 800-38F §6.3) via AWS-LC.
fn wrap_key(k_content: &[u8; 32], ss: &[u8; 48]) -> Result<Vec<u8>> {
    let mut wrap_key_bytes = derive_wrap_key(ss);
    let kek = AesKek::new(&AES_256, &wrap_key_bytes)
        .map_err(|_| anyhow::anyhow!("AES-256 KEK construction failed"))?;
    let mut output = vec![0u8; WRAPPED_KEY_LEN];
    let wrapped = kek.wrap_with_padding(k_content.as_ref(), &mut output)
        .map_err(|_| anyhow::anyhow!("AES-256-KWP wrap failed"))?;
    wrap_key_bytes.zeroize();
    debug_assert_eq!(wrapped.len(), WRAPPED_KEY_LEN);
    Ok(wrapped.to_vec())
}

fn unwrap_key(wrapped: &[u8], ss: &[u8; 48]) -> Result<[u8; 32]> {
    if wrapped.len() != WRAPPED_KEY_LEN {
        bail!("wrapped_key must be {} bytes, got {}", WRAPPED_KEY_LEN, wrapped.len());
    }
    let mut wrap_key_bytes = derive_wrap_key(ss);
    let kek = AesKek::new(&AES_256, &wrap_key_bytes)
        .map_err(|_| anyhow::anyhow!("AES-256 KEK construction failed"))?;
    let mut output = vec![0u8; 32];
    let plaintext = kek.unwrap_with_padding(wrapped, &mut output)
        .map_err(|_| CryptoError::KwpIntegrityFail)?;
    wrap_key_bytes.zeroize();
    let arr: [u8; 32] = (&*plaintext).try_into()
        .map_err(|_| CryptoError::KeyLength)?;
    output.zeroize(); // M-6: clear the Vec<u8> holding K_content before dealloc
    Ok(arr)
}

fn encrypt_body(plaintext: &[u8], k_content: &ContentKey, aad: &[u8]) -> Result<EnvelopeBody> {
    let rng = SystemRandom::new();
    let mut nonce_bytes = [0u8; 12];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| anyhow::anyhow!("RNG failed generating nonce"))?;

    let unbound = UnboundKey::new(&AES_256_GCM, &k_content.0)
        .map_err(|_| anyhow::anyhow!("invalid AES-256-GCM key"))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut in_out = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut in_out)
        .map_err(|_| anyhow::anyhow!("AES-256-GCM encryption failed"))?;

    Ok(EnvelopeBody { nonce: nonce_bytes, ciphertext: in_out })
}

fn decrypt_body(body: &EnvelopeBody, k_content: &ContentKey, aad: &[u8]) -> Result<Vec<u8>> {
    let unbound = UnboundKey::new(&AES_256_GCM, &k_content.0)
        .map_err(|_| anyhow::anyhow!("invalid AES-256-GCM key"))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::assume_unique_for_key(body.nonce);

    let mut in_out = body.ciphertext.clone();
    let plaintext = key.open_in_place(nonce, Aad::from(aad), &mut in_out)
        .map_err(|_| CryptoError::AeadAuthFail)?;

    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kem::generate_keypair;

    const TEST_FP: &[u8; 32] = b"test-fingerprint-32-bytes-padded";

    fn seal_simple(
        plaintext: &[u8],
        sender_id: Uuid,
        sender_key_id: Uuid,
        sender_sk: &SenderSigningKey,
        expires: u64,
        recipients: &[(Uuid, Uuid, &RecipientPublicKey)],
    ) -> Result<(EnvelopeHeader, EnvelopeBody)> {
        let r: Vec<_> = recipients.iter().map(|(rid, kid, pk)| (*rid, *kid, *pk, TEST_FP.as_ref())).collect();
        seal(plaintext, sender_id, sender_key_id, sender_sk, TEST_FP, expires, false, &r)
    }

    fn open_simple(
        header: &EnvelopeHeader,
        body: &EnvelopeBody,
        recipient_id: Uuid,
        sk: &RecipientSecretKey,
        pk: &RecipientPublicKey,
        sender_vk: &SenderVerifyingKey,
    ) -> Result<Vec<u8>> {
        // Retrieve the key_id from the slot so the test helper stays in sync with the header.
        let key_id = header.recipients.iter()
            .find(|s| s.recipient_id == recipient_id)
            .map(|s| s.recipient_key_id)
            .unwrap_or_else(Uuid::new_v4);
        open(header, body, recipient_id, key_id, sk, pk, sender_vk, TEST_FP, TEST_FP)
    }

    #[test]
    fn seal_open_roundtrip() {
        let (pk, sk) = generate_keypair().unwrap();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let key_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let plaintext = b"hello aegis encrypted delivery";
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal_simple(
            plaintext,
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            &[(recipient_id, key_id, &pk)],
        )
        .unwrap();

        assert_eq!(header.sender_signature.len(), 64);
        let decrypted = open_simple(&header, &body, recipient_id, &sk, &pk, &sender_vk).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (pk1, sk1) = generate_keypair().unwrap();
        let (pk2, _sk2) = generate_keypair().unwrap();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let rid1 = Uuid::new_v4();
        let rid2 = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal_simple(
            b"secret",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            &[(rid1, Uuid::new_v4(), &pk1)],
        )
        .unwrap();

        let result = open_simple(&header, &body, rid2, &sk1, &pk2, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let (pk, sk) = generate_keypair().unwrap();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, mut body) = seal_simple(
            b"tamper me",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        body.ciphertext[0] ^= 0xff;
        let result = open_simple(&header, &body, recipient_id, &sk, &pk, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn tampered_signature_rejected() {
        let (pk, sk) = generate_keypair().unwrap();
        let (sender_vk, sender_sk) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (mut header, body) = seal_simple(
            b"sign me",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        header.sender_signature[0] ^= 0xff;
        let result = open_simple(&header, &body, recipient_id, &sk, &pk, &sender_vk);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_sender_key_rejected() {
        let (pk, sk) = generate_keypair().unwrap();
        let (_sender_vk, sender_sk) = generate_sender_keypair();
        let (wrong_vk, _) = generate_sender_keypair();
        let recipient_id = Uuid::new_v4();
        let sender_id = Uuid::new_v4();
        let sender_key_id = Uuid::new_v4();
        let expires = chrono::Utc::now().timestamp_millis() as u64 + 86_400_000;

        let (header, body) = seal_simple(
            b"who sent this?",
            sender_id,
            sender_key_id,
            &sender_sk,
            expires,
            &[(recipient_id, Uuid::new_v4(), &pk)],
        )
        .unwrap();

        let result = open_simple(&header, &body, recipient_id, &sk, &pk, &wrong_vk);
        assert!(result.is_err());
    }

    /// ECDSA with aws-lc-rs uses random nonces (FIPS 186-5 §6.4 approves both
    /// deterministic and random). Verify sign+verify roundtrip instead.
    #[test]
    fn ecdsa_sign_verify_roundtrip() {
        let (vk, sk) = generate_sender_keypair();
        let rng = SystemRandom::new();
        let msg = b"aegis-v1-sender-sig test message";
        let sig = sk.0.sign(&rng, msg).expect("signing must not fail");
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &vk.0)
            .verify(msg, sig.as_ref())
            .expect("verification must succeed");
    }
}
