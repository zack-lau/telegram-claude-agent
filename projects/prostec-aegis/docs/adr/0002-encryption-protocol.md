# ADR 0002: Aegis Encryption Protocol

**Status:** Accepted  
**Date:** 2026-05-08  
**Deciders:** Zack  
**Inputs:** Opus 4.7 cryptography review, Qwen3.6 security review, IETF research (RFC 9180, RFC 9807, draft-ietf-hpke-pq)

---

## Context

Aegis is an encrypted delivery layer. AI agents produce work product; human recipients authenticate to decrypt and access their deliveries. The server must be unable to read plaintext content even under a full database compromise or legal compulsion. Senders need cryptographic assurance that a delivery reaches the intended recipient and no one else. Recipients need cryptographic assurance that a delivery genuinely came from the claimed sender.

This ADR defines the full encryption stack: wire format, key management, private key storage, sender authentication, forward secrecy, and key directory design.

---

## Decisions

### D1: HPKE (RFC 9180) as the outer framework

**Decision:** Use HPKE (RFC 9180) as the cryptographic envelope. Do not build a bespoke hybrid.

HPKE composes KEM + KDF + AEAD in a standardized, audited way. It provides:
- Principled KDF binding — no hand-rolled `HKDF(ss1 || ss2)` mistakes
- Suite agility — swap KEM without rewriting framing (critical as PQ standards evolve)
- Native Auth mode for sender binding
- Free single-shot and exporter modes

The `hpke` Rust crate (implementing RFC 9180) is the foundation. The hybrid KEM is implemented as a custom `Kem` trait on top of RustCrypto primitives, tracking `draft-ietf-hpke-pq`. This is structural plumbing around standard primitives, not invented crypto.

**Risk flagged (Qwen):** draft-ietf-hpke-pq is not yet an RFC and could change. Mitigation: the KEM combiner is isolated behind a trait. When the draft is finalized or the `hpke` crate ships PQ support, swap the KEM implementation without touching the envelope format.

---

### D2: Hybrid KEM — MLKEM768_P256

**Decision:** ML-KEM-768 (FIPS 203) + P-256 ECDH (FIPS 186-5), combined per the draft-ietf-hpke-pq combiner pattern.

**Not X-Wing (ML-KEM-768 + X25519).** X25519 is not in FIPS 186-5. Enterprise procurement will reject it before reading the design. For a B2B SaaS serving regulated industries, FIPS compliance on the classical component is non-negotiable.

**Suite identifier:** `HPKE(KEM=MLKEM768_P256, KDF=HKDF-SHA384, AEAD=AES-256-GCM-256)`

SHA-384 + AES-256 chosen for CNSA 2.0 parameter alignment — 192-bit-class KEM paired with 128-bit AEAD mismatches in FIPS audit optics even if secure in practice.

**KEM combiner construction:**

```
// Encapsulation (sender side)
ct_pq, ss_pq   = ML-KEM-768.Encap(pk_kem_recipient)           // 1088 B ct, 32 B ss
sk_ec_eph, pk_ec_eph = P256.GenerateEphemeral()               // 33 B compressed pk
ss_ec          = P256.ECDH(sk_ec_eph, pk_ec_recipient)         // 32 B ss

shared_secret = HKDF-SHA384(
  ikm  = ss_pq || ss_ec,
  salt = "",
  info = "HPKE-v1 KEM MLKEM768_P256 shared_secret"
          || ct_pq || pk_ec_eph || pk_ec_recipient_kem || pk_ec_recipient
)
// Transcript binding includes both ciphertexts and both public keys.
// This is the X-Wing insight: without it, the combiner is not IND-CCA secure
// if one component breaks. Do NOT omit the ciphertext/pk terms from info.

encap = ct_pq (1088 B) || pk_ec_eph (33 B)   // 1121 bytes total
```

```
// Decapsulation (recipient side)
ct_pq    = encap[0..1088]
pk_ec_eph = encap[1088..1121]

ss_pq = ML-KEM-768.Decap(ct_pq, sk_kem_recipient)
ss_ec = P256.ECDH(sk_ec_recipient, pk_ec_eph)

shared_secret = HKDF-SHA384(
  ikm  = ss_pq || ss_ec,
  salt = "",
  info = <same as encap, reconstruct from envelope>
)
```

**Rust crates:**
```toml
ml-kem   = "0.3"    # RustCrypto, ACVP-verified, constant-time
p256     = "0.13"   # RustCrypto, ECDH via elliptic-curve feature
hkdf     = "0.12"
aes-gcm  = "0.10"
hpke     = "0.11"   # RFC 9180 outer frame; hybrid KEM plugged in as custom Kem impl
```

**Security review flag:** The KEM combiner transcript binding must be reviewed by a cryptographer before launch. Specifically verify: (a) info derivation includes all required terms, (b) constant-time properties preserved through the combiner, (c) KDF separation between KEM and AEAD key derivation.

---

### D3: Delivery Envelope Wire Format

**Decision:** Per-delivery content key (`K_content`) wrapped separately per recipient. One `body` AEAD. All components cryptographically bound.

```
DeliveryEnvelope {
    // Header (included as AAD on body AEAD and info on each recipient KEM wrap)
    version:        u8,                    // = 1
    suite_id:       u16,                   // HPKE suite (MLKEM768_P256 = 0x0040 proposed)
    content_id:     [u8; 32],              // random, addresses body blob in S3/object store
    sender_id:      [u8; 16],              // sender UUID
    sender_key_id:  [u8; 16],              // which sender keypair signed/authed
    created_at:     u64,                   // Unix epoch ms
    expires_at:     u64,                   // delivery expiry; body deleted after this
    burn_after_read: bool,                  // if true, body deleted on first successful decrypt

    // Recipients — one entry per addressed recipient
    recipients: [
        RecipientSlot {
            recipient_id:    [u8; 16],     // recipient UUID
            recipient_key_id: [u8; 16],    // which recipient keypair was used
            encap:           [u8; 1121],   // HPKE encapsulation output (KEM ciphertext)
            wrapped_key:     [u8; 48],     // HPKE.SealAuth(K_content) → 32B key + 16B AEAD tag
        }
    ],

    // Body (stored separately in object store, referenced by content_id)
    body: AES-256-GCM(
        key  = K_content,
        iv   = random 96-bit nonce,
        aad  = SHA-384(canonical(header_fields_above)),
        data = plaintext_payload
    )
}
```

**Why content key + per-recipient wraps (Option B):**
- Uniform pipeline for N=1 and N=20 — same code path
- Late recipient add: re-wrap `K_content` for new recipient without re-encrypting body
- Storage efficiency at N>1: body encrypted once regardless of recipient count
- Integrity: body AEAD's AAD binds to recipient set + sender + suite → server cannot swap recipients, drop a recipient, or substitute a different body without breaking decryption

**`K_content` key derivation:** `K_content` is a fresh 32-byte random key generated per delivery. It is NOT derived from the KEM — it is wrapped BY the KEM. This avoids the footgun where you reuse the HPKE-derived key for one recipient and wrap-only for others.

**Body storage:** `body` ciphertext lives in S3 (or equivalent object store) keyed by `content_id`. The envelope header (with recipient slots) lives in DynamoDB. Separating them limits what a DynamoDB read alone reveals.

---

### D4: Sender Authentication — HPKE Auth Mode

**Decision:** Use HPKE `mode_auth` for sender binding. Transport-layer API key auth is necessary but not sufficient.

**Why crypto-layer sender auth matters:** Transport auth (`API key → Aegis`) authenticates the API caller to the server. It does NOT bind the ciphertext to a sender identity. An Aegis insider with DynamoDB write access could swap the `sender_id` field in an envelope row, and there would be no cryptographic check. With HPKE Auth mode, the recipient's `OpenAuth()` call requires the claimed sender's static public key — wrong sender = AEAD tag failure = delivery rejected.

For Aegis's core value proposition ("AI work product addressed to human"), the recipient must have cryptographic assurance of WHICH agent produced the delivery, not just "Aegis asserts it was Agent X."

**Qwen concern addressed:** Qwen flagged that "sender key compromise breaks confidentiality." This is incorrect. In HPKE Auth mode, compromising `sk_sender` lets an attacker FORGE new messages that appear to come from that sender (until key rotation), but does NOT decrypt existing ciphertexts (you still need `sk_recipient`). The threat is sender impersonation after key compromise, not confidentiality loss. Mitigation: sender key rotation is explicit in the key directory (§D7) and old Auth-mode ciphertexts retain their authentication proof against the key version they were encrypted with.

**Anonymous deliveries:** For one-time or anonymous sender use cases, allow `mode_base` with `sender_authenticated: false` flag in the envelope. Recipients see a clear UI indicator.

**Separate Ed25519 signature (Qwen alternative):** NOT adopted. HPKE Auth mode provides sender binding as part of the AEAD — adding a separate Ed25519 signature would be redundant code, an additional primitive to implement and audit, and introduces the "sign-then-encrypt vs encrypt-then-sign" ordering question unnecessarily. HPKE Auth handles it correctly by construction.

---

### D5: Private Key Storage — OPAQUE-Wrapped Server-Side Blob

**Decision:** Recipient private keys are generated client-side and stored server-side as AES-256-GCM blobs encrypted under an OPAQUE-derived key. Server never sees the password or the plaintext private key.

**Why not pure client-side (Option A):**
- Web browser: "device" is an IndexedDB origin. Clear site data → keys gone → all historical deliveries unreadable. Enterprise users WILL do this.
- No multi-device support without a separate device-pairing protocol.
- Account recovery is impossible — unacceptable for B2B ("lose your laptop, lose three years of legal deliveries").

**Why not key splitting (Option C):** Threshold schemes add operational complexity (share rotation, recovery ceremony) without solving the recovery problem in the web context. Overkill for user-bound keypairs at SaaS scale.

**OPAQUE construction (RFC 9807, standardized 2024):**
```
Registration:
  1. Client generates (sk_id, pk_id) — ML-KEM-768 + P-256 keypair bundle
  2. Client runs OPAQUE registration with password
  3. OPAQUE produces export_key (never leaves client)
  4. k_wrap = HKDF-SHA384(export_key, "aegis-v1-sk-wrap")
  5. enc_sk = AES-256-GCM(k_wrap, sk_id || pk_id, aad="aegis-v1-key-bundle")
  6. Client uploads: pk_id (public), enc_sk (encrypted blob)
  7. Server stores enc_sk — cannot decrypt without k_wrap — cannot derive k_wrap without password

Login:
  1. Client runs OPAQUE login with password
  2. OPAQUE produces export_key (same as registration, assuming correct password)
  3. k_wrap = HKDF-SHA384(export_key, "aegis-v1-sk-wrap")
  4. Client fetches enc_sk from server
  5. sk_id = AES-256-GCM-Decrypt(k_wrap, enc_sk)
  6. sk_id held in memory for session duration only — never written to disk/storage
```

**Recovery code (mandatory at enrollment):**
```
recovery_code = 20 random words (BIP-39 or similar, ~200 bits entropy)
k_recovery    = HKDF-SHA384(recovery_code, "aegis-v1-recovery-wrap")
enc_sk_recovery = AES-256-GCM(k_recovery, sk_id || pk_id, aad="aegis-v1-recovery")
// Server stores enc_sk_recovery alongside enc_sk
// Recovery: user enters code, derives k_recovery, decrypts sk_id
```

**Password rotation:** OPAQUE export_key changes when password changes. Client must re-derive `k_wrap`, re-encrypt `sk_id` to new `k_wrap`, and upload new `enc_sk`. This is done client-side; server swaps the blob atomically. Old `enc_sk` must be deleted immediately to prevent password-change bypass.

**Security guarantee (honest framing):** "Aegis cannot read your deliveries unless it can guess your password. Forgotten password without a recovery code = permanent loss of old deliveries." Do NOT market as "true E2E" — that would be misleading. Market as "zero-knowledge server."

**Qwen additions applied:** Enforce 12+ character password at client. OPAQUE provides offline-attack resistance (server can't precompute), but weak passwords still vulnerable post-server-breach. Rate-limit OPAQUE login attempts server-side. Account lockout after N failed attempts.

**Rust crate:** `opaque-ke` (Meta/Novi, Ristretto255-SHA-512 suite) — reference implementation of RFC 9807.

**Note on FIPS:** OPAQUE is authentication, not data-protection. The data at rest is protected by AES-256-GCM + HKDF-SHA384 (FIPS-compliant). Document this distinction in compliance materials.

---

### D6: Forward Secrecy for Async Delivery

**Decision:** Per-delivery ephemeral HPKE encapsulation provides sender-side FS. No prekeys. No server re-encryption. Recipient-side FS achieved via download-and-delete semantics and key rotation with forward-only effect.

**Sender-side FS:** HPKE Base/Auth mode generates a fresh ephemeral KEM keypair per `Seal()`. Ephemeral private key destroyed post-encap. Compromise of sender state after delivery cannot decrypt past deliveries. This is free with HPKE — no prekeys needed.

**Recipient-side FS — the hard case:** The recipient's long-term `sk_id` is needed to decap `K_content`. If `sk_id` is compromised, all stored ciphertext addressed to that `pk_id` is readable. This is fundamental to async delivery — you cannot have FS against long-term recipient key compromise without either prekeys (breaking the "pick up delivery weeks later" use case) or server-mediated proxy re-encryption (research-grade, not RFC-grade).

**Why prekeys don't fit Aegis:**
- Prekeys require the recipient to be online enough to replenish the bundle. Legal counsel opening a quarterly delivery cannot be expected to have pre-uploaded 100 one-time prekeys.
- Signal can do prekeys because phones are always online. Aegis recipients are episodic users.
- Exhausted prekey bundle = sender cannot deliver = worse than no FS.

**Why server re-encryption is rejected:**
- Proxy re-encryption schemes that compose with HPKE + PQ are research-grade. Do not ship research crypto.
- Server-assisted re-encryption requires server to briefly hold plaintext OR participate in the KEM (in which case server could decrypt). Neither is acceptable.
- "Client-side re-encryption on login" (Qwen suggestion) is theoretically interesting but complex: client downloads old ciphertext, decrypts, re-encrypts with fresh key, re-uploads. Requires old `sk_id` to be retained for the re-encryption, which means the key isn't actually rotated until re-encryption completes. Deferred to v2 research item.

**Forward secrecy in practice — three mechanisms:**

1. **Burn-after-read mode (per-delivery opt-in):**
   - Ciphertext deleted from Aegis storage on first successful `OpenAuth()` client-side.
   - Sender sets `burn_after_read: true` in envelope.
   - Provides practical FS: past deliveries are gone from server after recipient reads them.
   - Default for sensitive deliveries.

2. **Auto-expiry:** `expires_at` in envelope. Body deleted from object store after expiry. Configurable per tenant (default 90 days). Limits the FS gap window.

3. **Key rotation (forward-only):**
   - Recipient may rotate `(sk_id, pk_id)` at any time.
   - Old `pk_id` marked `status=retired` in directory. New deliveries use new `pk_id`.
   - Stored ciphertext under old `pk_id` remains decryptable until recipient explicitly purges old key from their OPAQUE blob.
   - Purging old key = permanent loss of unread deliveries under that key — recipient must confirm with explicit UI acknowledgment.
   - Compromise response: mark `pk_id` `status=revoked` with timestamp. All ciphertext `created_at > revoked_at` is rejected at decrypt-time client check. Ciphertext `created_at < revoked_at` was legitimately encrypted — no cryptographic recovery possible.

---

### D7: Key Directory Design

**Decision:** Signed key bundles, Aegis-CA-rooted, with versioned rotation and optional Key Transparency log for high-assurance tenants.

**Key bundle schema:**
```rust
struct KeyBundle {
    subject_id:         Uuid,           // recipient or sender UUID
    subject_type:       SubjectType,    // Recipient | SenderHuman | SenderAgent
    tenant_id:          Uuid,
    pk_kem:             [u8; 1184],     // ML-KEM-768 public key
    pk_ec:              [u8; 33],       // P-256 public key (compressed)
    suite_ids:          Vec<u16>,       // suites this key supports
    key_id:             [u8; 16],       // SHA-256(pk_kem || pk_ec)[0..16]
    prev_key_id:        Option<[u8;16]>,// chain to previous key (rotation audit trail)
    created_at:         u64,
    not_before:         u64,
    not_after:          u64,            // hard expiry, default created_at + 365d
    status:             KeyStatus,      // Active | Retired | Revoked
    retired_at:         Option<u64>,
    revoked_at:         Option<u64>,
    revocation_reason:  Option<RevocationReason>,
}

struct KeyBundleSigned {
    bundle:         KeyBundle,
    signature:      Vec<u8>,   // ECDSA-P-384 over SHA-384(canonical_cbor(bundle))
    ca_cert_chain:  Vec<Vec<u8>>, // DER-encoded certs to Aegis Root CA
}
```

**CA and signing:**
- Aegis maintains an intermediate CA (ECDSA-P-384) signed by an Aegis Root CA (offline, HSM-backed).
- All key bundles are signed by the intermediate CA after the subject submits `pk_kem || pk_ec` over an authenticated channel (post-Cognito login + OPAQUE).
- Use ECDSA-P-384 for the CA signature — independent of the data-protection P-256, avoids conflating auth and encryption key usage.
- Future: add ML-DSA-65 (FIPS 204) co-signature when FIPS 140-3 module validations catch up. Schema already supports `signature` as `Vec<u8>` to accommodate concatenated signatures.
- **Tenant CA delegation (enterprise):** Large tenants can have their own intermediate CA signed by Aegis Root. Their IT can audit/issue without Aegis seeing employees' raw public keys.

**Sender verification flow:**
```
1. Sender authenticates (API key over TLS)
2. Sender requests: GET /directory/{tenant_id}/recipients/{email}
3. Server returns KeyBundleSigned
4. Sender verifies:
   a. Signature chains to pinned Aegis Root CA
   b. status == Active
   c. now ∈ [not_before, not_after]
   d. suite_ids intersection with sender's supported suites is non-empty
5. Sender picks strongest mutually supported suite
6. Sender records recipient_key_id in delivery envelope
7. Recipient, on decrypt, loads the sk_id corresponding to recipient_key_id
```

**Rotation policy:**
- Default lifetime: 12 months (`not_after = created_at + 365d`)
- Auto-rotate at 11 months: new bundle generated, old key `status=Active` for 30-day overlap, then `status=Retired`
- Forced rotation triggers: password change (optional, recommended), device loss, explicit user request, revocation event
- Revocation: `status=Revoked`, `revoked_at`, `revocation_reason` set. Senders MUST refuse to encrypt to revoked keys. Bundle cache TTL ≤ 1h for revocation propagation.

**Key Transparency log (v2, high-assurance tenants):**
- Merkle tree of `(subject_id, key_id, operation, ts)` entries, append-only, published.
- Recipients can verify "all keys ever attributed to my subject_id" — detects a malicious Aegis silently issuing a second key for a recipient.
- Schema designed now (`prev_key_id`, deterministic `key_id` derivation) to enable CT log without compatibility break.
- Apple Contact Key Verification / WhatsApp KT are the reference implementations.

---

## Security Review Checklist (Before Shipping Crypto Layer)

These must be reviewed by a cryptographer, not just tested:

- [ ] KEM combiner transcript binding — verify all required terms (both shared secrets, both ciphertexts, both public keys) included in `info`
- [ ] Constant-time guarantees through the P-256 + ML-KEM combiner in Rust
- [ ] KDF separation — HPKE's internal KDF and the combiner KDF must use domain-separated labels
- [ ] AAD wiring across body AEAD and per-recipient wraps — verify body can't be decrypted under a different recipient set
- [ ] OPAQUE export_key derivation parameters — verify `info` string uniqueness per use
- [ ] Key bundle canonicalization for signing — classic serialization footgun
- [ ] `mode_auth` KEM transcript in Rust `hpke` crate when custom KEM is plugged in
- [ ] `enc_sk` re-encryption on password change is atomic (old blob deleted only after new blob confirmed)

---

## Implementation Order

**Phase 1 — Core delivery (ship first):**
1. MLKEM768_P256 KEM trait implementation (Rust, ~200 lines, plugs into `hpke` crate)
2. Key directory: bundle schema, Aegis CA setup, signing endpoint
3. Delivery envelope: `DeliveryEnvelope` struct, `seal()` / `open_auth()`
4. Body object store (S3) + envelope DynamoDB persistence
5. Sender API: authenticate, fetch recipient bundle, seal delivery, submit
6. Recipient API: list deliveries, fetch envelope, open_auth (client-side decrypt)

**Phase 2 — Key management:**
7. OPAQUE registration + login (`opaque-ke` crate integration)
8. Client-side keygen + `enc_sk` upload
9. Key rotation flow
10. Recovery code enrollment

**Phase 3 — Hardening:**
11. Burn-after-read + auto-expiry
12. Revocation + revocation propagation to sender cache
13. Tenant CA delegation
14. Key Transparency log (high-assurance tier)

---

## References

- RFC 9180 — Hybrid Public Key Encryption (HPKE)
- RFC 9807 — The OPAQUE Asymmetric PAKE Protocol
- draft-ietf-hpke-pq — Post-Quantum KEMs for HPKE
- draft-connolly-cfrg-xwing-kem — X-Wing hybrid KEM (informational, reference for combiner pattern)
- NIST FIPS 203 — ML-KEM
- NIST FIPS 186-5 — P-256
- NIST CNSA 2.0 Suite — parameter guidance
- RustCrypto `ml-kem`, `p256`, `hkdf`, `aes-gcm` crates
- `opaque-ke` crate (Meta/Novi, RFC 9807 reference implementation)
