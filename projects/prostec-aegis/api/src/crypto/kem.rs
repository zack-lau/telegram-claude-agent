// MLKEM768_P256 hybrid KEM combiner per ADR 0003 / NIST SP 800-56C Rev.2.
//
// Dual-PRF cascade: ss_ec and ss_pq are each extracted separately, then
// combined via HKDF-Extract(salt=PRK_ec, ikm=PRK_pq). Transcript binding
// (ciphertext + public keys) in the Expand info ensures IND-CCA2 security.
// FIPS-approvable: P-256 ECDH satisfies the "at least one approved KEM" rule
// from SP 800-56C Rev.2; ML-KEM-768 is independently FIPS 203 approved.
//
// FIPS note: every primitive runs through AWS-LC (FIPS 140-3 #4816 path):
// ML-KEM-768 (FIPS 203) with SP 800-90A DRBG, ECDH P-256, HKDF-SHA384, HMAC-SHA384.

use anyhow::Result;
use aws_lc_rs::{
    agreement::{self, agree, agree_ephemeral, EphemeralPrivateKey, PrivateKey, UnparsedPublicKey},
    hmac,
    hkdf::{self, Prk, HKDF_SHA384},
    kem::{Ciphertext as KemCiphertext, DecapsulationKey as KemDecapKey,
          EncapsulationKey as KemEncapKey, AlgorithmId as KemAlgId, ML_KEM_768},
    rand::SystemRandom,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::util::push_lv;

/// Byte layout of the encapsulation output:
///   [0..1088]    ML-KEM-768 ciphertext
///   [1088..1121] P-256 ephemeral public key (compressed SEC1, 33 bytes)
pub const ENCAP_LEN: usize = 1088 + 33;

const ML_KEM_768_CT_LEN: usize = 1088;

/// Transcript constants bound to this suite/version.
pub const ENVELOPE_VERSION: &[u8] = b"aegis-v1";
pub const SUITE_ID: &[u8] = b"P256+MLKEM768+SHA384+AES256KWP";

/// Combined shared secret (before it feeds into HPKE KDF).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SharedSecret(pub [u8; 48]); // HKDF-SHA384 output

pub struct RecipientPublicKey {
    pub kem: KemEncapKey<KemAlgId>,
    /// P-256 public key, SEC1 compressed (33 bytes).
    pub ec: Vec<u8>,
    /// Raw ML-KEM-768 encapsulation key bytes (1184 B) for transcript binding.
    pub kem_bytes: Vec<u8>,
}

pub struct RecipientSecretKey {
    pub kem: KemDecapKey<KemAlgId>,
    pub ec: PrivateKey,
}

/// Compress a 65-byte uncompressed SEC1 P-256 point to 33 bytes.
fn compress_p256(uncompressed: &[u8]) -> [u8; 33] {
    assert_eq!(uncompressed.len(), 65, "expected 65-byte uncompressed SEC1");
    assert_eq!(uncompressed[0], 0x04, "expected uncompressed SEC1 prefix");
    let x = &uncompressed[1..33];
    let y_parity = uncompressed[64] & 1;
    let mut out = [0u8; 33];
    out[0] = 0x02 | y_parity;
    out[1..].copy_from_slice(x);
    out
}

/// HKDF-Extract via HMAC-SHA384: Extract(salt, ikm) = HMAC-SHA384(key=salt, data=ikm).
fn hkdf_extract_sha384(salt: &[u8], ikm: &[u8]) -> [u8; 48] {
    let key = hmac::Key::new(hmac::HMAC_SHA384, salt);
    let tag = hmac::sign(&key, ikm);
    tag.as_ref().try_into().expect("HMAC-SHA384 output is 48 bytes")
}

/// Encapsulate: produce (encap bytes, shared_secret) from recipient public keys.
pub fn encapsulate(
    pk_kem: &KemEncapKey<KemAlgId>,
    pk_kem_bytes: &[u8],
    pk_ec: &[u8], // 33-byte compressed SEC1
    sender_sk_fp: &[u8],
    recipient_aik_fp: &[u8],
) -> Result<([u8; ENCAP_LEN], SharedSecret)> {
    let rng = SystemRandom::new();

    // ML-KEM-768 encapsulation via AWS-LC FIPS DRBG.
    let (ct_pq, ss_pq) = pk_kem.encapsulate()
        .map_err(|_| anyhow::anyhow!("ML-KEM-768 encapsulate failed"))?;
    let ct_pq_bytes = ct_pq.as_ref();
    if ct_pq_bytes.len() != ML_KEM_768_CT_LEN {
        anyhow::bail!("unexpected ML-KEM-768 ciphertext length {}", ct_pq_bytes.len());
    }

    // P-256 ephemeral ECDH.
    let eph_sk = EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| anyhow::anyhow!("P-256 key generation failed"))?;
    let eph_pk_raw = eph_sk.compute_public_key()
        .map_err(|_| anyhow::anyhow!("P-256 compute_public_key failed"))?;
    let eph_pk_compressed = compress_p256(eph_pk_raw.as_ref());

    let ss_ec = agree_ephemeral(
        eph_sk,
        UnparsedPublicKey::new(&agreement::ECDH_P256, pk_ec),
        anyhow::anyhow!("ECDH encap failed — invalid peer public key"),
        |ss_bytes: &[u8]| Ok::<Vec<u8>, anyhow::Error>(ss_bytes.to_vec()),
    )?;

    let shared_secret = combine_secrets(
        ss_pq.as_ref(),
        &ss_ec,
        sender_sk_fp,
        recipient_aik_fp,
        pk_ec,
        pk_kem_bytes,
        &eph_pk_compressed,
        ct_pq_bytes,
    )?;

    let mut encap = [0u8; ENCAP_LEN];
    encap[..ML_KEM_768_CT_LEN].copy_from_slice(ct_pq_bytes);
    encap[ML_KEM_768_CT_LEN..].copy_from_slice(&eph_pk_compressed);

    Ok((encap, shared_secret))
}

/// Decapsulate: recover shared_secret from encap bytes + recipient secret keys.
pub fn decapsulate(
    encap: &[u8; ENCAP_LEN],
    sk_kem: &KemDecapKey<KemAlgId>,
    sk_ec: &PrivateKey,
    pk_ec: &[u8], // 33-byte compressed SEC1
    pk_kem_bytes: &[u8],
    sender_sk_fp: &[u8],
    recipient_aik_fp: &[u8],
) -> Result<SharedSecret> {
    let ct_pq_bytes = &encap[..ML_KEM_768_CT_LEN];
    let pk_eph_bytes = &encap[ML_KEM_768_CT_LEN..]; // 33-byte compressed SEC1

    // ML-KEM-768 decapsulate. Implicit rejection per FIPS 203 — wrong key
    // returns a deterministic but unrelated shared secret, never an error.
    let ss_pq = sk_kem.decapsulate(KemCiphertext::from(ct_pq_bytes))
        .map_err(|_| anyhow::anyhow!("ML-KEM-768 decapsulate failed"))?;

    let ss_ec = agree(
        sk_ec,
        UnparsedPublicKey::new(&agreement::ECDH_P256, pk_eph_bytes),
        anyhow::anyhow!("ECDH decap failed — invalid ephemeral public key"),
        |ss_bytes: &[u8]| Ok::<Vec<u8>, anyhow::Error>(ss_bytes.to_vec()),
    )?;

    combine_secrets(
        ss_pq.as_ref(),
        &ss_ec,
        sender_sk_fp,
        recipient_aik_fp,
        pk_ec,
        pk_kem_bytes,
        pk_eph_bytes,
        ct_pq_bytes,
    )
}

/// SP 800-56C Rev.2 dual-PRF cascade combiner with 8-field transcript binding.
fn combine_secrets(
    ss_pq: &[u8],
    ss_ec: &[u8],
    sender_sk_fp: &[u8],
    recipient_aik_fp: &[u8],
    recipient_ecdh_pubkey: &[u8],
    recipient_mlkem_pubkey: &[u8],
    ecdh_ephemeral_pubkey: &[u8],
    mlkem_ciphertext: &[u8],
) -> Result<SharedSecret> {
    let zero_salt = [0u8; 48];

    let prk_ec = hkdf_extract_sha384(&zero_salt, ss_ec);
    let prk_pq = hkdf_extract_sha384(&zero_salt, ss_pq);
    let prk_combined = hkdf_extract_sha384(&prk_ec, &prk_pq);

    let label = b"aegis-v1 kem combiner";
    let mut info = Vec::new();
    info.extend_from_slice(label);
    push_lv(&mut info, ENVELOPE_VERSION);
    push_lv(&mut info, SUITE_ID);
    push_lv(&mut info, sender_sk_fp);
    push_lv(&mut info, recipient_aik_fp);
    push_lv(&mut info, recipient_ecdh_pubkey);
    push_lv(&mut info, recipient_mlkem_pubkey);
    push_lv(&mut info, ecdh_ephemeral_pubkey);
    push_lv(&mut info, mlkem_ciphertext);

    struct Len48;
    impl hkdf::KeyType for Len48 {
        fn len(&self) -> usize { 48 }
    }
    let prk = Prk::new_less_safe(HKDF_SHA384, &prk_combined);
    let mut okm = [0u8; 48];
    prk.expand(&[info.as_slice()], Len48)
        .expect("prk_combined is valid SHA-384 length")
        .fill(&mut okm)
        .expect("48 bytes is valid for HKDF-SHA384");

    Ok(SharedSecret(okm))
}

/// Generate a fresh ML-KEM-768 + P-256 keypair.
pub fn generate_keypair() -> Result<(RecipientPublicKey, RecipientSecretKey)> {
    let dk = KemDecapKey::generate(&ML_KEM_768)
        .map_err(|_| anyhow::anyhow!("ML-KEM-768 key generation failed"))?;
    let ek = dk.encapsulation_key()
        .map_err(|_| anyhow::anyhow!("ML-KEM-768 encapsulation key extraction failed"))?;
    let kem_bytes = ek.key_bytes()
        .map_err(|_| anyhow::anyhow!("ML-KEM-768 encapsulation key serialization failed"))?
        .as_ref()
        .to_vec();

    let ec_sk = PrivateKey::generate(&agreement::ECDH_P256)
        .map_err(|_| anyhow::anyhow!("P-256 key generation failed"))?;
    let ec_pk_raw = ec_sk.compute_public_key()
        .map_err(|_| anyhow::anyhow!("P-256 compute_public_key failed"))?;
    let ec_pk_compressed = compress_p256(ec_pk_raw.as_ref()).to_vec();

    Ok((
        RecipientPublicKey { kem: ek, ec: ec_pk_compressed, kem_bytes },
        RecipientSecretKey { kem: dk, ec: ec_sk },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encapsulate_test(pk: &RecipientPublicKey) -> ([u8; ENCAP_LEN], SharedSecret) {
        encapsulate(&pk.kem, &pk.kem_bytes, &pk.ec, b"", b"").unwrap()
    }

    fn decapsulate_test(
        encap: &[u8; ENCAP_LEN],
        sk: &RecipientSecretKey,
        pk: &RecipientPublicKey,
    ) -> Result<SharedSecret> {
        decapsulate(encap, &sk.kem, &sk.ec, &pk.ec, &pk.kem_bytes, b"", b"")
    }

    #[test]
    fn roundtrip_encap_decap() {
        let (pk, sk) = generate_keypair().unwrap();
        let (encap, ss_enc) = encapsulate_test(&pk);
        let ss_dec = decapsulate_test(&encap, &sk, &pk).unwrap();
        assert_eq!(ss_enc.0, ss_dec.0);
    }

    #[test]
    fn wrong_recipient_key_fails() {
        let (pk1, _sk1) = generate_keypair().unwrap();
        let (_pk2, sk2) = generate_keypair().unwrap();
        let (encap, ss_enc) = encapsulate_test(&pk1);
        // ML-KEM uses implicit rejection — always returns a value, but wrong bytes.
        let ss_dec = decapsulate(&encap, &sk2.kem, &sk2.ec, &pk1.ec, &pk1.kem_bytes, b"", b"")
            .unwrap();
        assert_ne!(ss_enc.0, ss_dec.0);
    }
}
