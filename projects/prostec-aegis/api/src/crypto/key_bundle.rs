// KeyBundle and KeyBundleSigned per ADR 0002 §D7.
//
// Schema: {recipient_id, kem_pk, ec_pk, version, expiry, signature}
// Signature covers the serialized bundle fields (excluding signature itself).
// Aegis CA signs with ECDSA-P-384 — longer term, rotate to a KT log.

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey, Signer, Verifier};
use p256::PublicKey as P256PublicKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Raw key bundle — serialized for signing and storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBundle {
    pub recipient_id: Uuid,
    /// ML-KEM-768 encapsulation key (1184 bytes), base64url-encoded in JSON.
    pub kem_pk: Vec<u8>,
    /// P-256 public key, SEC1 compressed (33 bytes).
    pub ec_pk: Vec<u8>,
    pub version: u8,
    pub expires_at: DateTime<Utc>,
}

/// Signed key bundle — what the key directory serves to senders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBundleSigned {
    pub bundle: KeyBundle,
    /// Ed25519 signature over `canonical_bytes(&bundle)`.
    pub signature: Vec<u8>,
    /// Fingerprint of the Aegis CA signing key used.
    pub signer_key_id: String,
}

impl KeyBundle {
    /// Canonical byte representation signed by the CA.
    /// All fields concatenated with length prefixes to avoid ambiguity.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(self.recipient_id.as_bytes());
        push_lv(&mut out, &self.kem_pk);
        push_lv(&mut out, &self.ec_pk);
        out.push(self.version);
        let ts = self.expires_at.timestamp().to_be_bytes();
        out.extend_from_slice(&ts);
        Ok(out)
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at < Utc::now()
    }
}

impl KeyBundleSigned {
    pub fn sign(bundle: KeyBundle, signing_key: &SigningKey, signer_key_id: &str) -> Result<Self> {
        let msg = bundle.canonical_bytes()?;
        let sig: Signature = signing_key.sign(&msg);
        Ok(Self {
            bundle,
            signature: sig.to_bytes().to_vec(),
            signer_key_id: signer_key_id.to_owned(),
        })
    }

    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<()> {
        if self.bundle.is_expired() {
            bail!("key bundle expired");
        }
        let msg = self.bundle.canonical_bytes()?;
        let sig_bytes: [u8; 64] = self.signature.as_slice().try_into()
            .map_err(|_| anyhow::anyhow!("invalid signature length"))?;
        let sig = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify(&msg, &sig)
            .map_err(|_| anyhow::anyhow!("key bundle signature verification failed"))
    }
}

fn push_lv(buf: &mut Vec<u8>, data: &[u8]) {
    let len = data.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey as Ed25519SigningKey;
    use rand::rngs::OsRng;

    fn make_bundle(recipient_id: Uuid) -> KeyBundle {
        KeyBundle {
            recipient_id,
            kem_pk: vec![0u8; 1184],
            ec_pk: vec![0u8; 33],
            version: 1,
            expires_at: Utc::now() + chrono::Duration::days(365),
        }
    }

    #[test]
    fn sign_and_verify() {
        let mut csprng = OsRng;
        let sk = Ed25519SigningKey::generate(&mut csprng);
        let vk = sk.verifying_key();
        let bundle = make_bundle(Uuid::new_v4());
        let signed = KeyBundleSigned::sign(bundle, &sk, "aegis-ca-v1").unwrap();
        assert!(signed.verify(&vk).is_ok());
    }

    #[test]
    fn tampered_bundle_fails_verify() {
        let mut csprng = OsRng;
        let sk = Ed25519SigningKey::generate(&mut csprng);
        let vk = sk.verifying_key();
        let bundle = make_bundle(Uuid::new_v4());
        let mut signed = KeyBundleSigned::sign(bundle, &sk, "aegis-ca-v1").unwrap();
        signed.bundle.version = 99; // tamper
        assert!(signed.verify(&vk).is_err());
    }

    #[test]
    fn expired_bundle_fails_verify() {
        let mut csprng = OsRng;
        let sk = Ed25519SigningKey::generate(&mut csprng);
        let vk = sk.verifying_key();
        let mut bundle = make_bundle(Uuid::new_v4());
        bundle.expires_at = Utc::now() - chrono::Duration::seconds(1);
        let signed = KeyBundleSigned::sign(bundle, &sk, "aegis-ca-v1").unwrap();
        assert!(signed.verify(&vk).is_err());
    }
}
