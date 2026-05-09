// MLKEM768_P256 hybrid KEM combiner per ADR 0003 / NIST SP 800-56C Rev.2.
//
// Dual-PRF cascade: ss_ec and ss_pq are each extracted separately, then
// combined via HKDF-Extract(salt=PRK_ec, ikm=ss_pq). Transcript binding
// (ciphertext + public keys) in the Expand info ensures IND-CCA2 security.
// FIPS-approvable: P-256 ECDH satisfies the "at least one approved KEM" rule
// from SP 800-56C Rev.2.

use anyhow::Result;
use hkdf::Hkdf;
use ml_kem::{
    Ciphertext, Decapsulate, DecapsulationKey, Encapsulate, EncapsulationKey, Kem, MlKem768,
};
use p256::{
    ecdh::EphemeralSecret,
    elliptic_curve::sec1::ToEncodedPoint,
    PublicKey as P256PublicKey, SecretKey as P256SecretKey,
};
use rand_core::OsRng;
use sha2::Sha384;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Byte layout of the encapsulation output:
///   [0..1088]  ML-KEM-768 ciphertext
///   [1088..1121] P-256 ephemeral public key (compressed, 33 bytes)
pub const ENCAP_LEN: usize = 1088 + 33;

/// Combined shared secret (before it feeds into HPKE KDF).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SharedSecret(pub [u8; 48]); // HKDF-SHA384 output

pub struct RecipientPublicKey {
    pub kem: EncapsulationKey<MlKem768>,
    pub ec: P256PublicKey,
}

pub struct RecipientSecretKey {
    pub kem: DecapsulationKey<MlKem768>,
    pub ec: P256SecretKey,
}

/// Encapsulate: produce (encap bytes, shared_secret) from recipient public keys.
pub fn encapsulate(
    pk_kem: &EncapsulationKey<MlKem768>,
    pk_ec: &P256PublicKey,
) -> Result<([u8; ENCAP_LEN], SharedSecret)> {
    // ML-KEM-768 encapsulation — getrandom feature provides this no-RNG variant
    let (ct_pq, ss_pq) = pk_kem.encapsulate();

    // P-256 ephemeral ECDH
    let sk_eph = EphemeralSecret::random(&mut OsRng);
    let pk_eph = P256PublicKey::from(&sk_eph);
    let ss_ec_bytes = sk_eph
        .diffie_hellman(pk_ec)
        .raw_secret_bytes()
        .to_vec();

    // Use compressed SEC1 (33 bytes) to fit ENCAP_LEN and keep transcript consistent.
    let pk_eph_enc = pk_eph.to_encoded_point(true);
    let pk_eph_bytes = pk_eph_enc.as_bytes();
    let pk_ec_enc = pk_ec.to_encoded_point(true);
    let pk_ec_bytes = pk_ec_enc.as_bytes();
    let ct_pq_bytes = ct_pq.as_ref();

    let shared_secret = combine_secrets(
        ss_pq.as_ref(),
        &ss_ec_bytes,
        ct_pq_bytes,
        pk_eph_bytes,
        pk_ec_bytes,
    )?;

    let mut encap = [0u8; ENCAP_LEN];
    encap[..1088].copy_from_slice(ct_pq_bytes);
    encap[1088..].copy_from_slice(pk_eph_bytes);

    Ok((encap, shared_secret))
}

/// Decapsulate: recover shared_secret from encap bytes + recipient secret keys.
pub fn decapsulate(
    encap: &[u8; ENCAP_LEN],
    sk_kem: &DecapsulationKey<MlKem768>,
    sk_ec: &P256SecretKey,
    pk_ec: &P256PublicKey,
) -> Result<SharedSecret> {
    let ct_pq_bytes = &encap[..1088];
    let pk_eph_bytes = &encap[1088..];

    // Ciphertext<MlKem768> is Array<u8, 1088>; TryFrom<&[u8]> validates length.
    let ct_pq = Ciphertext::<MlKem768>::try_from(ct_pq_bytes)
        .map_err(|_| anyhow::anyhow!("invalid ml-kem ciphertext length"))?;

    // Decapsulation returns SharedKey directly (implicit rejection on wrong key)
    let ss_pq = sk_kem.decapsulate(&ct_pq);

    let pk_eph = P256PublicKey::from_sec1_bytes(pk_eph_bytes)
        .map_err(|_| anyhow::anyhow!("invalid p256 ephemeral public key"))?;

    let ss_ec_bytes = p256::ecdh::diffie_hellman(sk_ec.to_nonzero_scalar(), pk_eph.as_affine())
        .raw_secret_bytes()
        .to_vec();

    let pk_ec_enc = pk_ec.to_encoded_point(true);
    let pk_ec_bytes = pk_ec_enc.as_bytes();

    combine_secrets(
        ss_pq.as_ref(),
        &ss_ec_bytes,
        ct_pq_bytes,
        pk_eph_bytes,
        pk_ec_bytes,
    )
}

/// SP 800-56C Rev.2 dual-PRF cascade combiner with transcript binding.
fn combine_secrets(
    ss_pq: &[u8],
    ss_ec: &[u8],
    ct_pq: &[u8],
    pk_eph: &[u8],
    pk_ec_recipient: &[u8],
) -> Result<SharedSecret> {
    // SHA-384 output length is 48 bytes; use as zero salt per SP 800-56C Rev.2.
    let zero_salt = [0u8; 48];

    // PRK_ec = HKDF-Extract(salt=zeros[48], ikm=SS_ECDH_P256)
    let (prk_ec, _) = Hkdf::<Sha384>::extract(Some(&zero_salt), ss_ec);

    // PRK_combined = HKDF-Extract(salt=PRK_ec, ikm=SS_ML-KEM-768)
    let (prk_combined, _) = Hkdf::<Sha384>::extract(Some(&prk_ec[..]), ss_pq);

    // Transcript info: label + length-prefixed ciphertext and public keys.
    // Length-prefixing prevents concatenation ambiguity between variable-length inputs.
    let label = b"aegis-v1 kem combiner";
    let mut info = Vec::with_capacity(
        label.len() + 2 + ct_pq.len() + 2 + pk_eph.len() + 2 + pk_ec_recipient.len(),
    );
    info.extend_from_slice(label);
    push_lv(&mut info, ct_pq);
    push_lv(&mut info, pk_eph);
    push_lv(&mut info, pk_ec_recipient);

    // SS_combined = HKDF-Expand(PRK_combined, info=transcript, L=48)
    let hk = Hkdf::<Sha384>::from_prk(&prk_combined[..])
        .expect("prk_combined is valid SHA-384 output length");
    let mut okm = [0u8; 48];
    hk.expand(&info, &mut okm)
        .expect("48 bytes is a valid output length for HKDF-SHA384");

    Ok(SharedSecret(okm))
}

fn push_lv(buf: &mut Vec<u8>, data: &[u8]) {
    let len = data.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(data);
}

/// Generate a fresh ML-KEM-768 + P-256 keypair.
pub fn generate_keypair() -> (RecipientPublicKey, RecipientSecretKey) {
    let (dk, ek) = MlKem768::generate_keypair(); // (DecapsulationKey, EncapsulationKey)
    let ec_sk = P256SecretKey::random(&mut OsRng);
    let ec_pk = ec_sk.public_key();
    (
        RecipientPublicKey { kem: ek, ec: ec_pk },
        RecipientSecretKey { kem: dk, ec: ec_sk },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encap_decap() {
        let (pk, sk) = generate_keypair();
        let (encap, ss_enc) = encapsulate(&pk.kem, &pk.ec).unwrap();
        let ss_dec = decapsulate(&encap, &sk.kem, &sk.ec, &pk.ec).unwrap();
        assert_eq!(ss_enc.0, ss_dec.0);
    }

    #[test]
    fn wrong_recipient_key_fails() {
        let (pk1, _sk1) = generate_keypair();
        let (_pk2, sk2) = generate_keypair();
        let (encap, ss_enc) = encapsulate(&pk1.kem, &pk1.ec).unwrap();
        // Decap with wrong EC key — ss should differ (ml-kem uses implicit rejection)
        let ss_dec = decapsulate(&encap, &sk2.kem, &sk2.ec, &pk1.ec);
        if let Ok(ss) = ss_dec {
            assert_ne!(ss_enc.0, ss.0);
        }
    }
}
