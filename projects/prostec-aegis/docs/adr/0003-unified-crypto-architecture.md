# ADR 0003 — Unified Cryptographic Architecture (Mobile + Web, FIPS-Aligned)

**Status:** Accepted  
**Date:** 2026-05-08  
**Supersedes:** ADR 0002 §D2 (KEM combiner), §D5 (key storage)  
**Context:** Three architectural decisions were made after full-system review:
1. Aegis must support both mobile (iOS/Android) and web clients
2. FIPS compliance is required from the start (target: FIPS 140-3 module-level on server, FIPS-approved algorithms throughout)
3. OPAQUE with P-256 cipher suite is the key storage mechanism for web clients

---

## D1 — Cryptographic Algorithm Suite (FIPS Baseline)

All cryptographic operations across server, mobile, and web clients use this suite:

| Primitive | Algorithm | FIPS Reference |
|---|---|---|
| Post-quantum KEM | ML-KEM-768 | FIPS 203 (2024) |
| Classical KEM | ECDH P-256 | SP 800-56A Rev.3 |
| KEM combiner | HKDF-SHA-384 dual-PRF cascade | SP 800-56C Rev.2 |
| Key wrapping | AES-256-KWP | SP 800-38F + RFC 5649 |
| Body AEAD | AES-256-GCM | SP 800-38D |
| Sender signature | ECDSA-P256 | FIPS 186-5 |
| Key derivation | HKDF-SHA-384 | SP 800-56C Rev.2 |
| Password KDF (web) | OPAQUE (P-256/SHA-256) | RFC 9807 + FIPS primitives |
| Hash | SHA-384 | FIPS 180-4 |
| CSPRNG | OS TRNG | platform-specific |

**Removed from ADR 0002**: X-Wing (uses X25519, not FIPS-approved). Replaced with P-256 + ML-KEM-768 using SP 800-56C Rev.2 combiner.

---

## D2 — KEM Combiner (FIPS SP 800-56C Rev.2 Compliant)

The hybrid shared secret is derived via a dual-PRF cascade including full transcript binding for IND-CCA2 security:

```
// Step 1: Extract from each KEM's shared secret
PRK_ec   = HKDF-Extract(salt = 0x00...0 [48 bytes], ikm = SS_ECDH_P256)
PRK_pq   = HKDF-Extract(salt = 0x00...0 [48 bytes], ikm = SS_ML-KEM-768)

// Step 2: Cascade combiner per SP 800-56C Rev.2
PRK_combined = HKDF-Extract(salt = PRK_ec, ikm = PRK_pq)

// Step 3: Expand with transcript binding (IND-CCA2)
// info = label || len(ct_pq) || ct_pq || len(pk_eph_ec) || pk_eph_ec || len(pk_ec_recipient) || pk_ec_recipient
SS_combined = HKDF-Expand(
    prk  = PRK_combined,
    info = "aegis-v1 kem combiner" || transcript_bytes,
    L    = 48
)
```

`SS_combined` is 48 bytes. The wrapping key is then derived with domain separation:
```
k_wrap = HKDF-Expand(SS_combined, info = "aegis-v1 envelope key-wrap", L = 32)
```

**Security rationale**: The cascade combiner achieves FIPS-compliant key derivation when at least one KEM is FIPS-approved (ECDH P-256 qualifies). The transcript binding ensures the combined key is IND-CCA2 secure against an adversary who controls either KEM output independently. Per the forthcoming SP 800-227, this construction is the intended hybrid KEM approach.

---

## D3 — Key Wrapping: AES-256-KWP

Key wrapping uses **AES-256-KWP** (RFC 5649, NIST SP 800-38F §6.3), not AES-256-GCM.

**Why not AES-GCM**:
- AES-GCM requires a 96-bit nonce; random nonce collision probability is non-negligible at scale (NIST retires AES-GCM keys after 2^32 random invocations)
- AES-KWP is deterministic — no nonce, no nonce reuse risk
- NIST SP 800-38F explicitly recommends KWP for key material of any size

**Wire format change from ADR 0002**: The `wrapped_key` field in `RecipientSlot` changes from `[12B nonce || 32B ct || 16B tag]` (60 bytes) to `[8B IV || 40B wrapped]` (48 bytes). This is a breaking wire format change; implement before any envelopes reach production.

```rust
// Wrap: AES-256-KWP
let kek = Kek::<Aes256>::new(&k_wrap.into());
let wrapped = kek.wrap_with_padding_vec(k_content)?; // 40 bytes output for 32-byte input

// Unwrap:
let k_content_bytes: [u8; 32] = kek
    .unwrap_with_padding_vec(wrapped)?
    .try_into()?;
```

Crate: `aes-kw = "0.2"` with `alloc` feature.

---

## D4 — Sender Authentication: ECDSA-P256 Signature

HPKE Auth mode (ADR 0002 §D4) is **replaced** with an explicit ECDSA-P256 signature. HPKE Auth has a Key Compromise Impersonation vulnerability (RFC 9180 §9.1): an adversary who obtains a *recipient's* private key can forge messages from any sender.

ECDSA-P256 is KCI-resistant: forging a sender's signature requires the sender's private key.

**Sender signature is over the canonical header bytes** (same SHA-384 hash used for AAD):

```
EnvelopeHeader.sender_signature = ECDSA-P256.sign(sk_sender, aad_bytes)
```

Where `aad_bytes = SHA-384(version || suite_id || content_id || sender_id || sender_key_id || created_at_ms || expires_at_ms || burn_after_read || for each slot: recipient_id || recipient_key_id || encap)`.

Verification at open time: before decrypting the body, verify the sender signature against the sender's public key from the key directory. Reject if invalid.

**Sender keypair**: P-256 ECDSA. Separate from the recipient KEM keypair. The `KeyBundle` includes both the KEM public key (for receiving) and the ECDSA public key (for verifying sent messages by this identity).

---

## D5 — Private Key Storage by Platform

### iOS (Secure Enclave)

Private keys are generated and stored in the Secure Enclave using `SecKeyCreateRandomKey` with `kSecAttrTokenIDSecureEnclave`. Keys are P-256 (the only curve Secure Enclave supports). The ML-KEM-768 keypair runs on the main processor within Apple's FIPS 140-3 validated corecrypto module.

```swift
let attributes: [String: Any] = [
    kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom,
    kSecAttrKeySizeInBits: 256,
    kSecAttrTokenID: kSecAttrTokenIDSecureEnclave,
    kSecPrivateKeyAttribs: [
        kSecAttrIsPermanent: true,
        kSecAttrApplicationTag: "cc.lzac.aegis.recipient-key"
    ]
]
```

ML-KEM-768 key material is wrapped using the Secure Enclave P-256 key (ECIES) and stored in the Keychain. Unwrapping happens in the Secure Enclave; decapsulation runs in corecrypto.

**No OPAQUE needed on mobile.** The hardware enforces key non-exportability.

### Android (StrongBox / BoringCrypto)

Keys are generated in Android Keystore with `setIsStrongBoxBacked(true)`. StrongBox (Google Titan M2 or equivalent) provides the hardware root of trust. Falls back to BoringCrypto TEE (still FIPS 140-3 validated) if StrongBox unavailable.

ML-KEM-768 key material is wrapped using the Keystore AES key and stored in app-private storage. BoringCrypto handles all crypto operations.

**No OPAQUE needed on mobile.**

### Web (OPAQUE + Server-Side Encrypted Blob)

Web clients cannot rely on hardware key storage. The private key blob is stored server-side, encrypted with a wrapping key derived from the user's password via OPAQUE.

**Protocol (registration):**
1. Client generates `KeyPair(KEM, ECDSA)` in browser memory
2. Client runs OPAQUE registration flow with server (password never transmitted)
3. Server stores OPAQUE credential (verifier, not password)
4. Client derives `k_wrap = OPAQUE_ExportKey` (48 bytes from RFC 9807 §4.1.2)
5. Client wraps private key: `blob = AES-256-KWP(k_wrap[..32], private_key_bytes)`
6. Client sends `blob` to server for storage. Server stores blob; never knows `k_wrap`.

**Protocol (login / key retrieval):**
1. Client runs OPAQUE login flow with server
2. Client recovers `k_wrap` from OPAQUE export key (same value as registration if password unchanged)
3. Client unwraps private key: `private_key_bytes = AES-256-KWP-Unwrap(k_wrap[..32], blob)`
4. Client holds private key in memory (non-extractable WebCrypto key if possible) for session duration
5. On session end: zeroize key from memory

**OPAQUE cipher suite**: `OPAQUE-P256-HKDF-SHA256` (P-256 group, SHA-256, HKDF). All primitives FIPS-approved. The OPRF step uses P-256 point multiplication and hash-to-curve (RFC 9380 simplified SWU) — pending explicit FIPS inclusion but built on approved components.

**Rust crate**: `opaque-ke` (Facebook Research), RFC 9807 synced as of July 2025. Pin to `opaque-ke = "2"` with P256 feature.

**Web crypto operations in browser**: The OPAQUE client-side blinding and unblinding use P-256 scalar multiplication. This can run via WebCrypto (`ECDH` key agreement) or a small WASM shim for hash-to-curve. All operations use FIPS-approved algorithms; the WASM shim is not from a FIPS 140-3 validated module (acceptable for web tier per §D6).

---

## D6 — FIPS Compliance Boundary

FIPS 140-3 validated modules are required for the server and mobile tiers:

| Tier | Module | FIPS 140-3 Validated |
|---|---|---|
| Aegis server (Rust/AWS) | AWS FIPS endpoints + Rust `aws-lc-rs` | ✓ (AWS-LC is FIPS 140-3 validated) |
| iOS | Apple corecrypto | ✓ |
| Android | Google BoringCrypto | ✓ |
| Web browser (ML-KEM) | WASM polyfill | ✗ — approved algorithms, non-validated module |
| Web browser (P-256/AES/HKDF) | Browser WebCrypto | ✓ (Safari corecrypto, Chrome BoringSSL in FIPS mode) |

**Web FIPS posture**: "FIPS-ready" — all algorithms are NIST-approved; ML-KEM runs in an unvalidated WASM module. This is acceptable for most enterprise/commercial contexts and FedRAMP Moderate. For DoD/IC environments, mobile is the compliant client path.

**Swappable ML-KEM interface**: The web client's ML-KEM implementation is behind a `KemProvider` interface. A future FIPS 140-3 validated browser native implementation (expected with WebCrypto ML-KEM support, ~2026-2027) can replace the WASM shim without changing the protocol.

**Server FIPS**: Replace `ring` with `aws-lc-rs` in `Cargo.toml`. AWS-LC has FIPS 140-3 certificates and covers AES-GCM, AES-KWP, HKDF, SHA-384, ECDH P-256, ECDSA P-256. ML-KEM-768 is included in AWS-LC via `aws-lc-rs` since v0.3.

---

## D7 — Envelope Wire Format Updates

`RecipientSlot` changes from ADR 0002:

| Field | ADR 0002 | ADR 0003 |
|---|---|---|
| `encap` | 1121 bytes (1088 ML-KEM + 33 P-256) | unchanged |
| `wrapped_key` | 60 bytes (12 nonce + 32 ct + 16 tag, GCM) | **40 bytes (8 IV + 32 wrapped, KWP)** |

`EnvelopeHeader` adds:
```json
{
  "sender_signature": "<base64, 64 bytes ECDSA-P256 DER signature over aad_bytes>"
}
```

`KeyBundle` adds:
```json
{
  "ecdsa_pk": "<base64, 65 bytes P-256 uncompressed public key for sender verification>"
}
```

Encoding: JSON (web-compatible). A future CBOR encoding (RFC 8949 deterministic mode) can be added as `suite_id = 0x0041` for mobile-to-mobile paths where deterministic encoding is required.

---

## D8 — Streaming Chunk Nonce (for future large-doc support)

When streaming encryption is implemented, chunk nonces must include session binding to prevent nonce reuse on upload interruption:

```
chunk_nonce = HKDF-Expand(
    prk  = CEK,
    info = "aegis-v1 chunk-nonce" || upload_uuid [16B] || uint64_be(chunk_index),
    L    = 12
)
```

`upload_uuid` is a fresh random UUID generated at the start of each upload session. This prevents nonce reuse if an upload is interrupted and resumed from chunk 0.

---

## Open Items (not resolved by this ADR)

- **Multi-device sync for web**: When a user adds a second web device, the private key blob must be re-wrapped for the new device. Options: (a) re-derive via OPAQUE on new device (user re-enters password); (b) device-to-device key transfer via existing device's Aegis session. Decision deferred.
- **Key recovery**: If a user loses all devices and forgets OPAQUE password, private key is unrecoverable (all encrypted messages permanently inaccessible). A recovery mechanism (e.g., recovery code = encrypted backup of k_wrap) needs separate design.
- **CBOR envelope**: Deferred to when mobile client is implemented. Suite ID `0x0041` reserved.
- **SP 800-227**: Forthcoming NIST publication on hybrid KEMs. When published, validate this ADR's combiner construction against it and update if needed.
