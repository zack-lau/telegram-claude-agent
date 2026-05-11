// AIK-signed key bundle hierarchy per ADR 0003 §D7 + architecture §"Signed Key Bundle Structure".
//
// Trust root is the recipient's Account Identity Key (P-256 ECDSA), NOT an Aegis CA.
// The bundle and each device entry are signed by the AIK. Senders pin the AIK fingerprint
// out-of-band and verify all signatures against it. A compromised key directory cannot
// substitute keys because it does not hold the AIK private key.
//
// FIPS path: ECDSA-P256 verify via aws-lc-rs (FIPS 140-3 #4816), SHA-256 inner hash.

use anyhow::{bail, Result};
use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::push_lv;

/// Per-device entry inside a recipient's key bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Stable device identifier (UUID or short string), assigned by the client at first registration.
    pub device_id: String,
    /// P-256 ECDH public key, SEC1 compressed (33 bytes). Used for hybrid KEM.
    pub ecdh_pubkey: Vec<u8>,
    /// P-256 ECDSA public key, SEC1 uncompressed (65 bytes). Used for sender-signature verification
    /// when this device originates a delivery.
    pub ecdsa_pubkey: Vec<u8>,
    /// ML-KEM-768 encapsulation key (1184 bytes).
    pub mlkem_pubkey: Vec<u8>,
    pub created_at: DateTime<Utc>,
    /// ECDSA-P256 signature from the AIK over `canonical_device_bytes(self)`. 64 bytes raw r||s.
    pub signed_by_identity: Vec<u8>,
}

impl DeviceEntry {
    /// Canonical bytes signed by the AIK. Length-prefixed concatenation, NO trailing signature.
    pub fn canonical_device_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_lv(&mut out, self.device_id.as_bytes());
        push_lv(&mut out, &self.ecdh_pubkey);
        push_lv(&mut out, &self.ecdsa_pubkey);
        push_lv(&mut out, &self.mlkem_pubkey);
        out.extend_from_slice(&self.created_at.timestamp().to_be_bytes());
        out
    }

    pub fn validate_lengths(&self) -> Result<()> {
        // device_id: non-empty, max 128 chars, printable ASCII only — no null bytes or
        // control characters that could corrupt DynamoDB keys or log lines (Q-M3).
        if self.device_id.is_empty() || self.device_id.len() > 128 {
            bail!("device_id must be 1..=128 bytes, got {}", self.device_id.len());
        }
        if self.device_id.bytes().any(|b| b < 0x20 || b == 0x7f) {
            bail!("device_id contains control characters or null bytes");
        }
        if self.ecdh_pubkey.len() != 33 {
            bail!("ecdh_pubkey must be 33 bytes (SEC1 compressed), got {}", self.ecdh_pubkey.len());
        }
        if self.ecdsa_pubkey.len() != 65 {
            bail!("ecdsa_pubkey must be 65 bytes (SEC1 uncompressed), got {}", self.ecdsa_pubkey.len());
        }
        if self.mlkem_pubkey.len() != 1184 {
            bail!("mlkem_pubkey must be 1184 bytes (ML-KEM-768), got {}", self.mlkem_pubkey.len());
        }
        if self.signed_by_identity.len() != 64 {
            bail!("signed_by_identity must be 64 bytes (ECDSA r||s), got {}", self.signed_by_identity.len());
        }
        Ok(())
    }
}

/// Recipient's signed key bundle. Stored in the key directory and served to senders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBundle {
    pub recipient_id: Uuid,
    /// AIK public key (P-256 ECDSA), SEC1 uncompressed (65 bytes). The trust root for this recipient.
    pub account_identity_pubkey: Vec<u8>,
    /// All registered devices for this recipient. Multi-device supported; MVP typically has 1.
    pub devices: Vec<DeviceEntry>,
    /// Monotonically increasing version. Used for rollback protection by senders.
    pub bundle_version: u64,
    /// Bundle signing time + 7 days. Senders MUST reject expired bundles regardless of version.
    pub bundle_expiry: DateTime<Utc>,
}

impl KeyBundle {
    /// Canonical bytes signed by the AIK at the bundle level.
    /// Length-prefixed concatenation. The bundle_signature is NOT part of the canonical bytes.
    pub fn canonical_bundle_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(self.recipient_id.as_bytes());
        push_lv(&mut out, &self.account_identity_pubkey);
        out.extend_from_slice(&u32::try_from(self.devices.len()).expect("device count fits u32").to_be_bytes());
        for d in &self.devices {
            push_lv(&mut out, &d.canonical_device_bytes());
            push_lv(&mut out, &d.signed_by_identity);
        }
        out.extend_from_slice(&self.bundle_version.to_be_bytes());
        out.extend_from_slice(&self.bundle_expiry.timestamp().to_be_bytes());
        out
    }

    pub fn is_expired(&self) -> bool {
        self.bundle_expiry < Utc::now()
    }

    /// SHA-256 over the canonical bundle bytes — for sender-side rollback hash check
    /// when bundle_version equals the previously seen version.
    pub fn bundle_hash(&self) -> [u8; 32] {
        use aws_lc_rs::digest::{Context, SHA256};
        let mut ctx = Context::new(&SHA256);
        ctx.update(&self.canonical_bundle_bytes());
        ctx.finish().as_ref().try_into().expect("SHA-256 is 32 bytes")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBundleSigned {
    pub bundle: KeyBundle,
    /// ECDSA-P256 signature from the AIK over `bundle.canonical_bundle_bytes()`.
    /// 64 bytes raw r||s (matches `ECDSA_P256_SHA256_FIXED`).
    pub bundle_signature: Vec<u8>,
}

impl KeyBundleSigned {
    /// Verify the bundle:
    /// 1. Not expired
    /// 2. `bundle_signature` valid under `bundle.account_identity_pubkey`
    /// 3. Each device's `signed_by_identity` valid under the same AIK
    /// 4. Per-field length sanity checks
    ///
    /// Sender-side rollback (`bundle_version` monotonic + bundle hash on equal version)
    /// is the agent SDK's responsibility — it requires comparing against pinned state and
    /// is therefore out of scope for this server-side verifier.
    pub fn verify(&self) -> Result<()> {
        if self.bundle.is_expired() {
            bail!("key bundle expired");
        }
        if self.bundle.account_identity_pubkey.len() != 65 {
            bail!(
                "AIK must be 65 bytes (SEC1 uncompressed), got {}",
                self.bundle.account_identity_pubkey.len()
            );
        }
        if self.bundle_signature.len() != 64 {
            bail!(
                "bundle_signature must be 64 bytes (ECDSA r||s), got {}",
                self.bundle_signature.len()
            );
        }
        if self.bundle.devices.is_empty() {
            bail!("bundle has no devices");
        }
        if self.bundle.devices.len() > 16 {
            bail!("bundle has too many devices ({}); cap is 16", self.bundle.devices.len());
        }
        // Reject duplicate device_ids — would cause ambiguous routing downstream
        // (Qwen Round 6 crypto LOW).
        let mut seen = std::collections::HashSet::with_capacity(self.bundle.devices.len());
        for d in &self.bundle.devices {
            if !seen.insert(&d.device_id) {
                bail!("bundle contains duplicate device_id: {}", d.device_id);
            }
        }

        let aik = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &self.bundle.account_identity_pubkey);

        aik.verify(&self.bundle.canonical_bundle_bytes(), &self.bundle_signature)
            .map_err(|_| anyhow::anyhow!("bundle_signature verification failed against AIK"))?;

        for (idx, dev) in self.bundle.devices.iter().enumerate() {
            dev.validate_lengths()
                .map_err(|e| anyhow::anyhow!("device[{}]: {}", idx, e))?;
            aik.verify(&dev.canonical_device_bytes(), &dev.signed_by_identity)
                .map_err(|_| anyhow::anyhow!("device[{}] signed_by_identity verification failed", idx))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::{
        rand::SystemRandom,
        signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
    };

    fn make_aik() -> (EcdsaKeyPair, Vec<u8>) {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, doc.as_ref()).unwrap();
        let pk = kp.public_key().as_ref().to_vec(); // 65 bytes uncompressed
        (kp, pk)
    }

    fn signed_bundle_with_devices(num_devices: usize) -> (KeyBundleSigned, Vec<u8>) {
        let rng = SystemRandom::new();
        let (aik_sk, aik_pk) = make_aik();
        let recipient_id = Uuid::new_v4();

        let mut devices = Vec::new();
        for i in 0..num_devices {
            let mut dev = DeviceEntry {
                device_id: format!("dev-{}", i),
                ecdh_pubkey: vec![0x02u8; 33],   // dummy compressed P-256
                ecdsa_pubkey: vec![0x04u8; 65],  // dummy uncompressed P-256
                mlkem_pubkey: vec![0u8; 1184],
                created_at: Utc::now(),
                signed_by_identity: vec![],
            };
            let sig = aik_sk.sign(&rng, &dev.canonical_device_bytes()).unwrap();
            dev.signed_by_identity = sig.as_ref().to_vec();
            devices.push(dev);
        }

        let bundle = KeyBundle {
            recipient_id,
            account_identity_pubkey: aik_pk.clone(),
            devices,
            bundle_version: 1,
            bundle_expiry: Utc::now() + chrono::Duration::days(7),
        };
        let bundle_sig = aik_sk.sign(&rng, &bundle.canonical_bundle_bytes()).unwrap();
        let signed = KeyBundleSigned {
            bundle,
            bundle_signature: bundle_sig.as_ref().to_vec(),
        };
        (signed, aik_pk)
    }

    #[test]
    fn sign_and_verify_single_device() {
        let (signed, _aik_pk) = signed_bundle_with_devices(1);
        signed.verify().expect("valid bundle must verify");
    }

    #[test]
    fn sign_and_verify_multi_device() {
        let (signed, _aik_pk) = signed_bundle_with_devices(3);
        signed.verify().expect("multi-device bundle must verify");
    }

    #[test]
    fn tampered_device_pubkey_fails_verify() {
        // Tampering any device pubkey changes canonical_bundle_bytes, so the bundle_signature
        // will fail before we even reach the per-device signed_by_identity check. Either path
        // is acceptable — the tamper MUST be rejected.
        let (mut signed, _) = signed_bundle_with_devices(2);
        signed.bundle.devices[0].ecdh_pubkey[5] ^= 0xff;
        assert!(signed.verify().is_err());
    }

    #[test]
    fn tampered_device_signature_only_fails_verify() {
        // Tamper just signed_by_identity on a device WITHOUT touching pubkeys/timestamps.
        // canonical_device_bytes excludes signed_by_identity but canonical_bundle_bytes
        // includes signed_by_identity (push_lv on it), so this still trips bundle_signature.
        let (mut signed, _) = signed_bundle_with_devices(2);
        signed.bundle.devices[1].signed_by_identity[0] ^= 0xff;
        assert!(signed.verify().is_err());
    }

    #[test]
    fn tampered_bundle_version_fails_verify() {
        let (mut signed, _) = signed_bundle_with_devices(1);
        signed.bundle.bundle_version = 99;
        assert!(signed.verify().is_err());
    }

    #[test]
    fn expired_bundle_fails_verify() {
        let (mut signed, _) = signed_bundle_with_devices(1);
        signed.bundle.bundle_expiry = Utc::now() - chrono::Duration::seconds(1);
        let err = signed.verify().unwrap_err().to_string();
        assert!(err.contains("expired"));
    }

    #[test]
    fn empty_devices_rejected() {
        let (mut signed, _) = signed_bundle_with_devices(1);
        signed.bundle.devices.clear();
        // Re-sign so the bundle_signature isn't the failure mode
        let rng = SystemRandom::new();
        let (aik_sk, aik_pk) = make_aik();
        signed.bundle.account_identity_pubkey = aik_pk;
        signed.bundle_signature = aik_sk
            .sign(&rng, &signed.bundle.canonical_bundle_bytes())
            .unwrap()
            .as_ref()
            .to_vec();
        let err = signed.verify().unwrap_err().to_string();
        assert!(err.contains("no devices"));
    }

    #[test]
    fn bundle_hash_changes_when_content_changes() {
        let (signed, _) = signed_bundle_with_devices(1);
        let h1 = signed.bundle.bundle_hash();
        let mut signed2 = signed.clone();
        signed2.bundle.bundle_version = 2;
        let h2 = signed2.bundle.bundle_hash();
        assert_ne!(h1, h2);
    }
}
