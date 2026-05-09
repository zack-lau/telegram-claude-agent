# Aegis System Architecture

## Overview

Prostec Labs Aegis is a zero-document-storage encrypted document delivery service. AI agents encrypt sensitive documents client-side before saving to the user's own cloud storage. The mobile app decrypts using hardware-backed keys. Aegis never stores user documents or private keys — it operates as a key directory and upload coordination service.

**What Aegis does store/process:** API keys, billing data, OAuth delegation metadata, audit logs, admin dashboard state, SSO identity metadata, and support records. These are operational necessities for running the service, not user document data.

## Encryption Scheme

Hybrid post-quantum, combining classical and lattice-based key encapsulation with hardware secure element integration:

- **Hardware layer — key hierarchy:**
  - **Account Identity Key:** P-256 ECDSA (in Secure Enclave/StrongBox) — long-lived, generated at account creation. Fingerprint source. Signs device key bundles. NEVER rotates unless compromised.
  - **Device ECDH key:** P-256 (in Secure Enclave/StrongBox) — for key exchange only. Rotated every 90 days.
  - **Device ECDSA key:** P-256 (in Secure Enclave/StrongBox) — for signing revocation tokens and device-origin assertions. Rotated every 90 days.
  - Both iOS Secure Enclave and Android StrongBox support P-256 for ECDH and ECDSA independently
- **Software layer:** ML-KEM-768 (Kyber, NIST FIPS 203) — post-quantum (NIST Security Category 3), runs in app memory. Upgrade path: ML-KEM-1024 (Category 5) available for defense/government deployments requiring higher security margins; envelope version negotiation supports algorithm agility.
- **Combination:** Dual-PRF combiner per NIST SP 800-56C Rev. 2 (labeled extract-and-expand), used to derive a per-recipient Key Encryption Key (KEK):
  1. `prk_ecdh = HKDF-Extract(salt=suite_id, ikm=ecdh_shared_secret)`
  2. `prk_mlkem = HKDF-Extract(salt=suite_id, ikm=mlkem_shared_secret)`
  3. `prk = HKDF-Extract(salt=prk_ecdh, ikm=prk_mlkem)` — proper dual-PRF combiner producing a single PRK of HashLen (32) bytes
  4. `KEK = HKDF-Expand(prk, info="aegis-v1" || suite_id || recipient_id || transcript_hash, L=32)` — where `recipient_id` is the recipient's Account Identity Key fingerprint (16-byte Crockford Base32 decoded to 10 bytes)
  - `suite_id` = `"Aegis-P256-ECDH-ML-KEM-768-HKDF-SHA256-AES256GCM"`
  - `transcript_hash` = `SHA-256(envelope_version || suite_id || sender_signing_key_fingerprint || recipient_account_identity_fingerprint || recipient_ecdh_pubkey || recipient_mlkem_pubkey || ecdh_ephemeral_pubkey || mlkem_ciphertext)` — binds key derivation to the full key exchange transcript including algorithm identifiers, both parties' identities, all public keys involved, and envelope version, preventing cross-session key reuse, identity misbinding, and algorithm downgrade
  - Domain separation ensures each KEM output is cryptographically isolated before combination
  - **Security claim:** Following the dual-PRF combiner pattern (Bindel, Brendel, Fischlin, Goncalves, Stebila, "Hybrid Key Encapsulation Mechanisms and Authenticated Key Exchange," 2019; Giacon, Heuer, Poettering, "KEM Combiners," 2018), the combined key remains secure as long as at least one component KEM remains unbroken. The cascade construction — where `prk_ecdh` serves as salt (HMAC key position) and `prk_mlkem` serves as IKM (HMAC message position) — provides the dual-PRF guarantee: if ECDH is secure, the pseudorandom salt ensures output security regardless of `prk_mlkem`; if ML-KEM is secure, the pseudorandom IKM ensures output security regardless of `prk_ecdh`. This is strictly stronger than concatenating both shared secrets into a single HKDF-Extract call, which would lose the independent dual-PRF guarantee. Per RFC 5869 §3.1, salt should be "a random (or pseudorandom) string of the length HashLen" — a PRK satisfies this exactly.
- **Content Encryption Key (CEK):** A random 32-byte symmetric key generated from the platform CSPRNG for each document. The CEK encrypts the document payload. It is NOT derived from KEM material.
- **Key Encryption Key (KEK):** The per-recipient key derived from the hybrid KEM combiner (step 4 above). Each recipient gets a unique KEK. The KEK wraps the CEK for that recipient.
- **Symmetric:** AES-256-GCM for document (payload) encryption using the CEK; AES-256-KWP (RFC 5649) for wrapping the CEK with each recipient's KEK

### Multi-Recipient Encryption Model — Canonical Construction

This is the ONE authoritative description of envelope construction ordering. All other sections reference this.

1. **Generate CEK:** For each document, generate one random 32-byte Content Encryption Key (CEK) from the platform CSPRNG.
2. **Build recipient stanzas:** For each recipient device:
   a. Perform P-256 ECDH + ML-KEM-768 to derive a per-recipient shared secret.
   b. Derive KEK from shared secret via the dual-PRF combiner above.
   c. Wrap CEK: `AES-256-KWP(KEK, CEK)` → `wrapped_cek` (stored in envelope per-recipient stanza).
3. **Assemble unsigned header:** Build the complete header with all fields populated EXCEPT `sender.signature` which is set to `null`. This includes: version, algorithms, recipients (with stanzas from step 2), sender.id (fingerprint), and metadata.
4. **Compute header hash:** `header_hash = SHA-256(canonical_cbor(unsigned_header))` — the unsigned header is the header with `sender.signature = null`.
5. **Encrypt payload:** `AES-256-GCM(CEK, nonce, plaintext, aad=header_hash)` — one ciphertext blob shared by all recipients.
6. **Compute sender signature:** `sender_signature = ECDSA-P256(agent_signing_key, "aegis-v1-sender-sig" || header_hash || SHA-256(nonce || ciphertext))` — signs over a domain-separated concatenation of the header hash and the payload hash. The signature cannot be part of the header hash because it does not exist yet at step 4. Preferred implementation uses deterministic ECDSA nonce generation per RFC 6979 to eliminate dependence on entropy source quality during signing.
7. **Finalize envelope:** Insert `sender_signature` into the header's `sender.signature` field. Final envelope = header (with signature populated) + encrypted payload (nonce + ciphertext).
8. **Decryption:** Each recipient unwraps their copy of the CEK using their KEK, then decrypts the shared payload ciphertext. Sender signature is verified by recomputing the header hash from the unsigned header (header with signature field set to null) and verifying against the payload hash.

**Construction order rationale:** Recipient stanzas MUST be built before payload encryption because the unsigned header (which contains recipient stanzas) is used as AAD for the payload AEAD. The sender signature is computed AFTER encryption because it must cover both the header hash and the payload hash — it cannot exist at the time the header hash is computed.

## Aegis Envelope v1 Format

Based on HPKE semantics (RFC 9180) + age-style multi-recipient stanzas + COSE binary encoding (RFC 9052).

**Canonical CBOR encoding:** All references to `canonical_cbor()` in this document use Deterministic Encoding per RFC 8949 §4.2 (Core Deterministic Encoding Requirements): map keys sorted in bytewise lexicographic order of their encoded form, preferred integer/float serializations, no indefinite-length encodings. This ensures identical bytes for identical data structures across all implementations, which is critical for header hash computation and bundle hash verification.

```
Aegis Envelope v1
├── Header (CBOR-encoded)
│   ├── version: uint (1)
│   ├── algorithms: {kem: "P256-ECDH+ML-KEM-768", kdf: "HKDF-SHA256", aead: "AES-256-GCM"}
│   ├── sender: {
│   │     id: sender_signing_key_fingerprint,
│   │     pubkey: P-256 point (65 bytes, self-declared sender public key),
│   │     signature: ECDSA-P256(sender_signing_key, "aegis-v1-sender-sig" || header_hash || SHA-256(nonce || ciphertext))
│   │       // signature is null during header_hash computation; populated after encryption (see Canonical Construction)
│   │       // signature encoded in raw fixed-width r||s format (64 bytes for P-256)
│   │   }
│   ├── recipients: [
│   │   {
│   │     id: recipient_key_fingerprint,
│   │     type: "device" | "recovery" | "hardware-key",
│   │     ecdh_ephemeral: P-256 point (65 bytes),
│   │     mlkem_ciphertext: ML-KEM-768 ct (1088 bytes),
│   │     wrapped_cek: AES-256-KWP(KEK, CEK) [RFC 5649]
│   │   }, ...
│   │ ]
│   ├── metadata: {
│   │     created: timestamp,
│   │     doc_id: uuid,
│   │     provenance: {  // optional
│   │       agent_id: string,
│   │       runtime_version: string,
│   │       session_id: string,
│   │       model_id: string
│   │     }
│   │   }
│   └── (no separate header_mac — AES-256-GCM authenticates header via AAD)
├── Payload
│   ├── nonce: 12 bytes
│   └── ciphertext: AES-256-GCM(CEK, nonce, plaintext, aad=SHA-256(canonical_cbor(unsigned_header)))
        // unsigned_header = header with sender.signature = null (see Canonical Construction)
```

**Key wrapping:** The `wrapped_cek` field uses AES-256-KWP (Key Wrap with Padding, RFC 5649) to wrap the random CEK with the recipient's KEK. AES-KWP adds authenticated wrapping with padding support over plain AES-KW (RFC 3394), providing ciphertext integrity protection. AES-256-GCM remains the AEAD for payload encryption where a random nonce is generated per envelope.

**Sender authentication:** Each sender (AI agent or user) has a registered P-256 ECDSA signing key pair. The `sender` field in the header contains the sender's key fingerprint and an ECDSA signature computed per the Canonical Construction (step 6). Sender signature verification follows a tiered policy:

- **Open mode** (default for Free/Professional tiers): Unknown senders are flagged with a warning banner but the envelope is decryptable. The user can inspect the sender key fingerprint and choose to trust or block. In open mode, any party who can obtain the recipient's public key bundle can create and deliver a validly encrypted envelope signed by their own key. This creates a content injection, spam, and phishing risk. Recipients in open mode should treat unknown-sender documents with the same caution as unsolicited email.
- **Strict mode** (Business/Enterprise tiers): Only pre-authorized sender keys can deliver. Unknown senders are rejected — the envelope is NOT decrypted. The recipient maintains an authorized sender list signed by their Account Identity Key.

This enables sender authenticity verification and authorized sender policies. Senders register their public signing key with the recipient via the key directory or out-of-band.

**Header authentication:** No separate `header_mac` field. The header hash is passed as AAD to AES-256-GCM, which provides built-in authentication of both the header and payload in a single operation.

**CBOR schema validation:** Decoders MUST enforce strict schema validation:
- Maximum envelope size: 50 MiB (reject before parsing; streaming CBOR parsing recommended for payload section)
- Maximum header size: 64 KiB
- Maximum recipient count: 256
- All fields validated against expected CBOR major types and lengths
- No indefinite-length encodings permitted
- Decoders must use safe CBOR libraries that reject duplicate map keys and malformed inputs

## Deduplication and Staleness Detection

Envelopes use idempotent document IDs for deduplication (tolerant of unordered delivery, agent restarts, and cloud sync delays). This is NOT true replay prevention — if the local deduplication database is lost (device reset, app reinstall), previously-seen envelopes will be processed again.

- **`doc_id`:** UUID v4, generated at encryption time. Uniquely identifies each envelope. Stored in the envelope header metadata.
- **`created`:** Envelope creation time (Unix seconds). Used for staleness detection. Stored in the envelope header metadata field.
- **Recipient-side dedup:** The mobile app maintains a set of seen `doc_id` values in its local SQLite database. Deduplication ordering:
  - The app MAY perform a fast preliminary check of `doc_id` against the seen set for performance (skip if already seen and previously verified).
  - For NEW `doc_id` values: the app MUST verify sender signature + AEAD payload authentication BEFORE inserting the `doc_id` into the seen set.
  - Invalid or unauthenticated envelopes MUST NOT pollute the dedup state.
  - This prevents deduplication DoS where an attacker sends invalid envelopes with legitimate `doc_id` values to block real deliveries.
  - If a `doc_id` has already been seen (and was previously authenticated), the envelope is silently skipped (deduplicated, not rejected with an error).
- **Staleness warning:** Envelopes with `created` older than 90 days are flagged as potentially stale (user warning, not hard rejection). This accounts for legitimate delayed delivery via cloud sync.
- **No sequence numbers:** The design deliberately avoids per-sender monotonic sequence numbers, which are fragile with unordered cloud delivery, multiple agent instances, and device resets.
- **Limitation:** Local DB loss (device reset without backup) resets deduplication state. Old envelopes in cloud storage will appear as new. The staleness check (90-day warning) partially mitigates this but does not prevent it.

## Key Hierarchy

Aegis uses a three-level key hierarchy that separates long-lived identity from rotatable device keys:

- **Account Identity Key** — long-lived P-256 ECDSA key, generated at account creation, stored in Secure Enclave/StrongBox. The fingerprint is derived from this key. It NEVER rotates (unless compromised — requires full re-verification by all senders).
- **Device Encryption Keys** — P-256 ECDH + ML-KEM-768, per device, rotated every 90 days. Signed by the Account Identity Key.
- **Device Signing Keys** — P-256 ECDSA per device, used for signing revocation tokens and device-origin assertions. The Account Identity Key signs the complete key bundle and device entries. Device signing keys are also signed by the Account Identity Key.

### Signed Key Bundle Structure

```
signed_key_bundle = {
  account_identity_pubkey: P-256 point,
  devices: [
    {
      device_id: string,
      ecdh_pubkey: P-256 point,
      ecdsa_pubkey: P-256 point,
      mlkem_pubkey: ML-KEM-768 pubkey,
      created: timestamp,
      signed_by_identity: ECDSA signature from account identity key
    }, ...
  ],
  bundle_version: uint (monotonically increasing),
  bundle_expiry: timestamp (bundle signing time + 7 days),
  bundle_signature: ECDSA signature from account identity key (covers all fields above)
}
```

**Rotation without fingerprint change:** Device key rotation generates new device keys signed by the account identity key. The fingerprint (derived from the account identity key) stays the same. Senders verify new device keys are signed by the pinned identity key — no re-verification needed for routine rotation.

**Bundle version rollback protection:** Agents store `(recipient_id, last_seen_bundle_version, last_seen_bundle_hash)` alongside pinned fingerprints. Agents MUST reject bundles with `bundle_version` lower than the last seen version to prevent replay of old valid bundles.

**Bundle expiry:** Each bundle includes a `bundle_expiry` timestamp (7 days from signing). Agents MUST reject expired bundles regardless of version number. This bounds rollback/staleness to at most 7 days for first-fetch senders who have no prior version history. A compromised key directory can still serve a superseded-but-unexpired bundle within this window. A future key transparency log would eliminate this residual risk. Recipients must re-sign their bundle at least every 7 days (automated by the mobile app in the background).

**Bundle hash verification:** When `bundle_version == last_seen_bundle_version`, agents MUST verify `SHA-256(canonical_cbor(signed_key_bundle)) == last_seen_bundle_hash`. If the content differs with the same version number, reject the bundle (tampering detected).

## Agent-Side Fingerprint Verification

How an AI agent verifies a recipient's key fingerprint before encrypting:

1. **Registration:** When a user registers, the Aegis mobile app displays their fingerprint derived from the **Account Identity Key**: truncated SHA-256 of the AIK public key (uncompressed P-256 point, 65 bytes), encoded as 16 Crockford Base32 characters (case-insensitive, 80 bits = 16 chars × 5 bits/char), displayed in groups of 4 for readability (e.g., `ABCD-EF12-3456-7890`).
2. **Distribution:** The user provides this fingerprint to anyone who will send them encrypted docs — email signature, business card, or direct communication to their AI agent operator.
3. **Agent setup (one-time per recipient):** When configuring an AI agent to send to a recipient, the human operator enters/confirms the recipient's fingerprint during agent setup. The agent SDK stores this as a pinned fingerprint along with `last_seen_bundle_version = 0` and `last_seen_bundle_hash = null`.
4. **Ongoing verification:** On every subsequent send, the agent SDK verifies the key bundle signature against the pinned account identity key fingerprint, checks `bundle_version >= last_seen_bundle_version`, verifies `bundle_expiry` is in the future, and verifies bundle hash when version matches. Mismatch = refuse to encrypt + alert operator.
5. **Device key rotation handling:** When device keys rotate, the bundle_version increments and the bundle is re-signed by the account identity key. Since the fingerprint is derived from the account identity key (which does not rotate), no re-verification is needed. The agent simply verifies the new bundle signature matches the pinned fingerprint.
6. **Account identity key compromise (rare):** If the account identity key itself must be replaced (compromise), the fingerprint changes and ALL senders must re-verify. This should be extremely rare.
7. **MVP scope:** Verification is manual (human confirms during setup). Future: automated verification via signed key bundles from a key transparency log.

## Key Directory Integrity (Preventing Aegis as MITM)

The key directory MUST NOT be a single point of trust. Aegis coordinates key lookup but cannot forge key bundles:

- **Signed key bundles:** The recipient's mobile app signs their complete public key bundle (all device keys) with the **Account Identity Key**. The bundle includes a monotonically increasing `bundle_version` and covers all device keys with individual per-device signatures from the identity key.
- **Key directory serves signed bundles:** When an agent fetches a recipient's public key via `GET /v1/public-key/{recipient_id}`, the response includes the raw signed bundle (see Key Hierarchy section for structure).
- **Agent SDK verification:** Before encrypting, the agent SDK verifies:
  1. `bundle_signature` against the pinned account identity key fingerprint
  2. Each device's `signed_by_identity` signature
  3. `bundle_version >= last_seen_bundle_version` (rollback protection)
  4. `bundle_expiry` is in the future (reject expired bundles)
  5. If `bundle_version == last_seen_bundle_version`: verify bundle hash matches cached hash (tamper detection)
  If any check fails, encryption is refused.
- **Security property:** Even a fully compromised Aegis server cannot substitute rogue public keys — it cannot forge a signature without the recipient's Account Identity Key. The agent's pinned fingerprint ensures end-to-end key authenticity independent of server trust. Rollback protection via monotonic bundle_version + expiry bounds rollback/staleness to at most 7 days for first-fetch senders who have no prior version history.

## Upload Coordination / OAuth Model

Aegis coordinates uploads to the recipient's cloud storage using delegated OAuth:

- **Recipient grants access during onboarding:** The recipient authorizes Aegis with a scoped OAuth refresh token (`drive.file` scope for Google Drive) during mobile app setup. This grants Aegis write access ONLY to the app-specific folder.
- **Upload flow:** When an agent calls `GET /v1/upload-endpoint/{recipient_id}`, Aegis uses the stored refresh token to generate a pre-signed upload URL (or short-lived access token) for the recipient's Aegis folder. The agent uploads ciphertext directly using this URL.
- **Recipient state:** This OAuth token is the ONE piece of recipient state Aegis holds beyond the key directory. It is stored encrypted at rest (AES-256-GCM, key in AWS KMS).
- **Revocation:** Recipients can revoke OAuth access at any time via their Google account (or other provider's security settings). Revoking stops new deliveries but does not affect already-delivered encrypted files.
- **Threat model acknowledgment:** A compromised Aegis server with access to stored OAuth tokens could write arbitrary files to recipients' Aegis folders. However, in strict mode these would be opaque encrypted blobs that the recipient's app would reject (invalid signature, unknown sender, failed decryption). In open mode, such files would be decryptable with an unknown-sender warning. This is documented as an accepted residual risk — the blast radius is limited to storage pollution and content injection/phishing risk (as open-mode recipients will decrypt unknown-sender envelopes with a warning), not data exposure.

## System Flow

**Critical design constraint:** Aegis NEVER sees plaintext document content. All encryption happens client-side in the AI agent's runtime environment. The Aegis service is a key directory + upload coordinator only.

```
AI Agent (client-side encryption) — follows Canonical Construction (see Multi-Recipient Encryption Model):
  1. Agent generates document content
  2. Agent calls Aegis SDK → fetches recipient's public key bundle from key directory
  3. Agent SDK verifies bundle signature, bundle_version, bundle_expiry, and bundle_hash (see Key Bundle section)
  4. Agent generates random CEK (32 bytes from CSPRNG)
  5. For each recipient device: hybrid KEM (P-256 ECDH + ML-KEM-768) → KEK → AES-256-KWP(KEK, CEK)
  6. Assemble unsigned header (sender.signature = null)
  7. Compute header_hash = SHA-256(canonical_cbor(unsigned_header))
  8. Encrypt document: AES-256-GCM(CEK, nonce, plaintext, aad=header_hash)
  9. Compute sender signature: ECDSA-P256(agent_signing_key, "aegis-v1-sender-sig" || header_hash || SHA-256(nonce || ciphertext))
  10. Insert sender signature into header, finalize envelope
  11. Agent uploads .aegis envelope directly to recipient's cloud storage
  12. Agent discards plaintext, CEK, and all key material from memory

Mobile App (client-side decryption):
  1. Authenticates user (biometric)
  2. Lists .aegis files from user's cloud storage
  3. Downloads encrypted envelope, performs preliminary dedup check (skip if doc_id already verified)
  4. Verifies sender signature: recompute header_hash from unsigned header (signature field = null), verify ECDSA over "aegis-v1-sender-sig" || header_hash || SHA-256(nonce || ciphertext). Apply sender policy (open mode: flag unknown senders; strict mode: reject)
  5. P-256 ECDH in Secure Enclave/StrongBox (hardware) → shared secret
  6. ML-KEM-768 decapsulation in software → shared secret
  7. Combines secrets via dual-PRF HKDF → KEK
  8. Unwraps CEK: AES-256-KWP-unwrap(KEK, wrapped_cek)
  9. AES-256-GCM decrypts document payload using CEK (verifies AAD = header_hash)
  10. Inserts doc_id into seen set (only after BOTH sender signature AND payload AEAD verification succeed)
  11. Displays to user
```

**What flows through the Aegis service:**
- Public key lookups (key directory)
- Upload coordination metadata (recipient storage endpoint, folder path)
- API authentication and rate limiting
- Audit log entries (agent ID, recipient ID, timestamp — no document content)

**What NEVER flows through the Aegis service:**
- Plaintext document content
- Private keys (ECDH, ECDSA, or ML-KEM)
- Content Encryption Keys (CEKs) or Key Encryption Keys (KEKs)
- Decrypted data of any kind

## Hardware Key Storage (3 Tiers)

### Tier 1 (default): Phone's built-in secure element

- iOS: Secure Enclave (Account Identity Key + Device ECDH key + Device ECDSA key, all P-256, none leave chip)
- Android: StrongBox preferred, TEE Keymaster as fallback (Account Identity Key + Device ECDH + Device ECDSA, all P-256)
- **Android hardware requirements:**
  - MVP requires hardware-backed keystore (StrongBox preferred, TEE Keymaster as fallback)
  - App checks `isInsideSecureHardware()` at key generation and displays security tier to user ("StrongBox" vs "TEE-backed")
  - Devices without any hardware-backed keystore (no StrongBox, no TEE) are not supported — app refuses to generate keys
- ML-KEM-768 private key encrypted at rest, wrapped by a symmetric key derived from a Secure Enclave/StrongBox P-256 ECDH operation against a device-local ephemeral point
- ML-KEM private key decrypted into memory ONLY for the duration of decapsulation, then immediately wiped via platform-native secure zeroing (see Memory Safety below)
- Full hardware isolation for ML-KEM is not possible until secure enclaves support post-quantum algorithms natively
- Zero extra hardware needed, every modern phone with hardware-backed keystore supports this

### Tier 2 (optional): External hardware keys via NFC/USB

- YubiKey 5 NFC, Nitrokey, smart cards, etc.
- For users who want keys on a separate physical device
- "Even if your phone is compromised, the key is on a separate device"
- **NFC authentication:** Before performing any cryptographic operation over NFC, the app verifies the hardware key's certificate chain (manufacturer CA → device cert). This prevents rogue hardware keys from being used. Note: commodity hardware keys (YubiKey, Nitrokey) do not perform arbitrary app attestation verification — authentication is one-directional (app verifies key, not key verifies app).

### Tier 3 (enterprise): AWS cloud-based key management

- AWS KMS for key backup wrapping ($1/user/month per CMK)
- AWS Nitro Enclaves for server-assisted decryption (zero-trust, ~$0.17/hr amortized)
- AWS KMS External Key Store (XKS) for "bring your own HSM" enterprise customers

## Agent Interface

### Primary: MCP Server (Aegis SDK — runs in agent's environment)

For Claude, Cursor, AI tools with native MCP support. The MCP tool is a **client-side SDK** that performs encryption locally — it calls the Aegis API only for key lookups and upload coordination, never sending plaintext to Aegis.

Tools:
- `aegis_encrypt_and_deliver(recipient_id, document_content, filename, metadata)` — encrypts locally in-process, then uploads ciphertext to recipient's storage via coordination endpoint. Plaintext never leaves the agent's runtime.
- `aegis_get_public_key(recipient_id)` — fetches recipient's composite public key from the Aegis key directory.
- `aegis_list_recipients()` — lists available recipients the agent is authorized to send to.
- `aegis_get_upload_endpoint(recipient_id)` — returns the storage endpoint and auth token for uploading ciphertext to the recipient's cloud folder.

### Secondary: REST API (Coordination only)

For GPT Actions, LangChain, AutoGen, any HTTP client. Agents using the REST API directly must implement client-side encryption using the Aegis cryptographic specification.

- `GET /v1/public-key/{recipient_id}` — fetch recipient's public key
- `GET /v1/upload-endpoint/{recipient_id}` — get pre-signed upload URL for recipient's storage
- `POST /v1/upload-complete` — notify Aegis that delivery is complete (for audit logging)
- OpenAPI spec for GPT Actions compatibility

**Note:** There is no `POST /v1/encrypt` endpoint. Aegis never accepts plaintext.

### API Security

- **Authentication:** API key per agent, issued via admin dashboard. Keys are scoped to specific recipients and operations.
- **RBAC:** Role-based access — `encrypt-only` (default agent role), `admin` (key management, recipient CRUD), `audit` (read-only logs).
- **Rate limiting:** Per-key rate limits (default: 60 req/min for encrypt, 10 req/min for key fetch). Burst allowance configurable per tier.
- **Input validation:** All inputs validated against strict schemas. `filename` restricted to `[a-zA-Z0-9._-]`, max 255 chars. `metadata` max 4 KiB JSON. Ciphertext upload size limit: 50 MiB.
- **No plaintext endpoints:** The Aegis API never accepts plaintext document content. Encryption is performed client-side by the SDK.
- **Allowlists:** Agent keys bound to IP allowlists (optional, recommended for enterprise).
- **Audit logging:** All API calls logged with agent ID, timestamp, recipient, operation, and outcome. Logs retained per compliance tier (30 days free, 1 year business, configurable enterprise).
- **TLS:** All endpoints require TLS 1.3. No plaintext fallback.

## Public Key Distribution

- Public key stored in user's cloud storage folder (e.g., `Google Drive/Aegis/pubkey.aegis`)
- Aegis key directory caches/indexes public keys for fast lookup by agents
- Agent fetches public key → encrypts locally → uploads ciphertext directly to recipient's storage
- Alternative: WKD (Web Key Directory) on user's domain for enterprise

### Out-of-band fingerprint verification (mandatory)

Public keys in cloud storage are vulnerable to substitution by anyone with folder access. To prevent MITM:

1. **First key exchange:** When a recipient shares their public key with a sender/agent, the recipient's app displays a verification fingerprint: truncated SHA-256 of the **Account Identity Key** (P-256 ECDSA public key), encoded as 16 alphanumeric characters (case-insensitive, ~80 bits of entropy). Displayed in groups of 4 for readability (e.g., `ABCD-EF12-3456-7890`). Since the fingerprint is derived from the long-lived Account Identity Key (not device keys), it remains stable across device key rotations. For high-security contexts (legal, medical, defense), a 6-word verification phrase (from a 2048-word BIP39-style list, ~66 bits) is also available as an alternative.
2. **Verification:** The sender/agent must verify this fingerprint via a separate channel before first use:
   - **In-app:** QR code scan between devices (preferred)
   - **Manual:** Recipient reads fingerprint aloud or sends via a different messaging channel
   - **Enterprise:** WKD/DANE provides implicit verification via DNS trust chain
3. **Trust-on-first-use (TOFU):** After initial verification, the fingerprint is cached. If the public key changes without re-verification, the client MUST warn and refuse to encrypt until the new key is re-verified.
4. **Key transparency log (future):** Append-only log of key-fingerprint bindings for auditability.

## Backup & Migration

### Per-device key pairs (Signal model)

- Each device generates its own device key set: P-256 ECDH (key exchange) + P-256 ECDSA (signing) + ML-KEM-768 (post-quantum KEM)
- The Account Identity Key (long-lived, on primary device) signs each device's key bundle
- Documents encrypted to ALL registered device public keys (multi-recipient envelope — each device gets its own wrapped CEK)
- Adding new device: Account Identity Key signs new device's key bundle, bundle_version increments, future documents encrypted to new device

### Recovery mechanisms (layered)

1. **Recovery Key** (optional, user-held): 24-word BIP39 mnemonic derives a recovery key pair. All document keys also wrapped to this. Stored OFFLINE by the user — never stored or transmitted by Aegis. Recommended for individual users who prefer a single backup artifact.
2. **Shamir's Secret Sharing** (recommended for business/enterprise): Split recovery capability into N shares, require K-of-N to reconstruct. Primary recovery mechanism for organizations — eliminates single-point-of-failure of a single recovery key.
3. **Passphrase-wrapped backup:** ML-KEM private key encrypted with Argon2id-derived key, stored in user's cloud storage.
4. **Nitro Enclave recovery** (enterprise): Wrapped key sent to attestation-verified Nitro Enclave for decryption.

### Key Revocation

- **Revocation tokens:** When a device is decommissioned, it publishes a signed revocation token to the user's cloud storage (`Aegis/revoked/<key_fingerprint>.revoke`). The token is signed by the device's own ECDSA signing key (proving ownership) or by a designated admin ECDSA key (for lost/stolen devices).
- **Aegis counter-signature:** Revocation tokens are also counter-signed by the Aegis timestamp service, producing a signed statement: "device X was revoked at time T." This is the one piece of verifiable state Aegis maintains — it makes revocation auditable even if the cloud storage folder is tampered with.
- **Limitation — best-effort enforcement:** Cloud-storage-based revocation is best-effort, not cryptographically enforced. An attacker controlling the cloud folder could delete revocation tokens. The Aegis counter-signature provides an independent revocation record that senders can verify, but enforcement depends on the sender/agent checking the revocation service before encrypting.
- **Pre-encryption check:** Before encrypting to any device key, the sender/agent MUST check both the cloud storage revocation folder AND the Aegis revocation timestamp service. Encryption to a revoked key is refused.
- **Revocation guarantee level:** Best-effort revocation verification; fully effective only when both the Aegis timestamp service and cloud storage are honest and available. A compromised or malicious Aegis service can withhold revocations (lie by omission). A future key transparency log would provide stronger guarantees by making revocation withholding detectable.
- **Revocation cache (fail-closed):** Agents cache the last successful revocation check result per device key, with a configurable TTL (default: 1 hour). The fail-closed policy applies to BOTH the Aegis timestamp service AND the cloud storage revocation folder:
  - If EITHER source is unreachable AND the cache is within TTL: use cached result (OK).
  - If EITHER source is unreachable AND the cache is past TTL: **MUST NOT encrypt** (fail-closed, no exceptions).
  - There is no grace period or circuit breaker — stale revocation state is never acceptable for encryption decisions.
- **High availability requirement:** The revocation timestamp service is on the critical path for all encryption operations under fail-closed policy. It MUST be deployed with high availability (multi-region, CDN-cached responses with short TTL). Outage of the timestamp service will block all new encryptions once agent caches expire.
- **Revocation propagation:** Multi-recipient envelopes skip revoked keys. Existing envelopes encrypted to revoked keys remain decryptable by other recipients but the revoked device cannot decrypt new documents, provided senders observe and enforce the revocation status before encrypting.
- **Recommended device key rotation (90 days):** Rotation generates new device encryption keys (ECDH + ML-KEM) and device signing keys (ECDSA), all signed by the Account Identity Key. The bundle_version increments monotonically. Future documents are encrypted to new keys only. Old keys are retained read-only for historical document access — old envelopes encrypted to old keys remain decryptable by those keys (this is by design; you don't lose access to old docs). Rotation limits the window of exposure: if a device key is compromised, only documents encrypted during that key's lifetime are affected. The Account Identity Key does NOT rotate — the user's fingerprint remains stable across device key rotations. The app prompts users when rotation is recommended. Enterprise tier supports automated rotation via policy.

### Account Identity Key Recovery

The Account Identity Key is hardware-bound (Secure Enclave/StrongBox) and CANNOT be backed up or exported. Identity continuity requires the old device. Three recovery scenarios:

1. **Old device available:** The old device's AIK signs the new device's key bundle. The account fingerprint is unchanged. All senders continue encrypting without re-verification. Seamless.
2. **Old device unavailable, recovery key exists:** A new AIK is generated on the new device — the fingerprint CHANGES. ALL senders must re-verify the new fingerprint. The recovery key (BIP39 mnemonic or Shamir reconstruction) only restores access to historical documents by unwrapping old CEKs. It does NOT restore the identity. Document access is restored; identity is reset.
3. **Old device unavailable, no recovery key:** All historical documents are permanently lost. A new AIK is generated, new fingerprint issued. Fresh start.

The AIK never leaves hardware. There is no mechanism to "transfer" an identity without the physical old device present.

### Migration flow

1. Set up new phone → app generates new per-device key pair (and new AIK if old device unavailable)
2. Authenticate (biometric + passphrase or recovery key)
3. Old device (if available) signs new device's bundle with its AIK (identity preserved)
4. If old device unavailable: recovery key or Shamir reconstruction unwraps historical CEKs, re-wraps to new device keys. New AIK created — fingerprint changes, all senders must re-verify.
5. Old device key revoked (revocation token published)

## Cloud Storage Support

| Priority | Provider | Status |
|----------|----------|--------|
| 1 | Google Drive | MVP |
| 2 | OneDrive | MVP |
| 3 | S3-compatible (AWS S3, MinIO) | Phase 2 |
| 4 | iCloud Drive | Phase 3 |

Abstracted via `AegisStorageProvider` interface — each provider implements upload/download/list/delete.

OAuth2 with PKCE for all providers. Minimum scope: app-specific folder access only (e.g., `drive.file` for Google).

## Tech Stack

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Mobile framework | Flutter | Yubico uses it; yubikit_openpgp exists; single codebase for iOS/iPad/Android |
| Classical crypto (hardware) | P-256 ECDH + P-256 ECDSA | Only curve supported by both iOS SE and Android StrongBox; separate keys for exchange vs signing |
| Post-quantum crypto | ML-KEM-768 (Kyber) | NIST standardized (FIPS 203), fast, proven |
| Symmetric encryption | AES-256-GCM (payload, keyed by CEK) + AES-256-KWP (CEK wrap, keyed by KEK) | Standard, hardware-accelerated on mobile; KWP adds authenticated wrapping with padding |
| KDF | HKDF-SHA256 | Standard, used in TLS 1.3 and HPKE |
| Envelope encoding | CBOR / COSE | Compact binary, extensible, standardized |
| Cloud storage | Google Drive API v3 | Launch provider, Flutter SDK available |
| Agent interface | MCP SDK (client-side) + REST coordination API | Client-side encryption SDK + key directory/upload coordination |
| Key backup KDF | Argon2id | Memory-hard, resists GPU attacks |
| Recovery phrase | BIP39 (24 words) | Established, user-friendly |
| Web version (Phase 2) | WebAuthn PRF + AES | Different crypto path, USB YubiKey only |

## Web Version (Phase 2)

- Browser CANNOT do P-256 ECDH with Secure Enclave or OpenPGP with hardware keys
- Viable approach: WebAuthn PRF extension (derives symmetric AES key from YubiKey passkey via USB)
- Bitwarden uses this exact pattern
- Web requires USB-connected YubiKey, no NFC
- Accept that web version uses different key type — maintain both per user or web-only users get PRF keys

## Security Properties

- **Content-blind (zero-content-knowledge):** cloud storage provider never sees plaintext; Aegis service never sees plaintext. Note: Aegis does see coordination metadata (who sends to whom, when, envelope sizes) — "zero-knowledge" applies strictly to document content, not to all information.
- **Hardware-bound:** classical private key never leaves secure element
- **Post-quantum resistant:** hybrid scheme protects against future quantum attacks
- **Ephemeral sender keys (limited unlinkability):** Ephemeral sender ECDH keys per document prevent cryptographic linking of documents to a persistent sender identity by the cloud storage provider. This is a narrow property — the cloud provider still observes uploader OAuth identity, timing, file sizes, and IP addresses. It does NOT hold against Aegis itself (sees coordination metadata). It also does NOT provide forward secrecy — if a recipient's long-term private key is later compromised, old envelopes can be decrypted because the sender's ephemeral public key is stored in the envelope.
- **Multi-recipient:** single document encrypted to multiple device keys efficiently
- **Zero-document-storage:** Aegis service holds no user documents or private keys (it does process API keys, billing, audit logs, and coordination metadata)
- **Verified key exchange:** out-of-band fingerprint verification prevents public key substitution attacks
- **Key hierarchy:** Account Identity Key (long-lived, hardware-bound, fingerprint source) + rotatable Device Keys (90-day rotation, signed by identity key). Bundle versioning + expiry + hash verification bounds rollback/staleness to at most 7 days.
- **Key lifecycle:** revocation tokens (counter-signed by Aegis timestamp service) + recommended 90-day device key rotation limits exposure window; fail-closed revocation cache (MUST NOT encrypt if either revocation source stale and unreachable)
- **Sender authentication:** ECDSA-P256 sender signatures in envelope header enable sender authenticity verification and authorized sender policies (tiered: open mode flags unknown senders, strict mode rejects them)

## Threat Model

### Threats mitigated

| Threat | Mitigation |
|--------|-----------|
| **Compromised cloud storage** (deletion, rollback, key substitution) | **Partially mitigated.** Key substitution: mitigated by fingerprint verification. Key bundle rollback: mitigated by bundle_version monotonic check + bundle expiry (7-day max). Document deletion: NOT mitigated (acknowledged DoS vector; recommend cloud versioning/backup). Revocation token deletion: partially mitigated by Aegis timestamp service counter-signatures. Document re-processing after local DB loss: partially mitigated by staleness detection (90-day warning), not fully prevented. Future transparency log provides append-only auditability. |
| **Lost/stolen mobile device** | SE-bound keys require biometric unlock; device revocation propagates to senders who check revocation status before encrypting. Propagation depends on sender-side cache freshness and service availability. Remote wipe may fail to publish a revocation token if the device is offline, destroyed, or the user lacks an alternate signing authority (Account Identity Key on another device). In such cases, revocation requires manual intervention via the Aegis dashboard or admin key. |
| **Compromised Aegis API key** | Limited blast radius — API keys can only look up public keys and coordinate uploads. No access to documents, private keys, or decryption capabilities |
| **Insider at Aegis** | Limited access — no documents transit the service, no private keys stored. Insider can access coordination metadata (who sent to whom, when) but not content |
| **Metadata leakage** (filenames, timestamps, sizes, access patterns) | Acknowledged risk. Envelope metadata is minimal (doc_id, timestamp). Filenames should be opaque UUIDs. Future: encrypted metadata field in envelope header |

### Threats acknowledged but out of scope

| Threat | Rationale |
|--------|-----------|
| **Compromised AI agent runtime** | The agent generates the plaintext — it inherently has access. Aegis encrypts the output; it cannot protect the generation process. This is a fundamental limit of any delivery encryption system. |
| **Compromised recipient device OS** | If the OS is compromised below the secure element abstraction, keys may be extractable. Aegis relies on platform security (iOS/Android) for the hardware trust boundary. |
| **Screenshot/clipboard leakage on recipient device** | Once decrypted and displayed, content is subject to OS-level exfiltration. DRM-style protections are outside Aegis's scope. |
| **Quantum attacks on P-256 (future)** | Mitigated by the hybrid scheme — ML-KEM-768 provides post-quantum security. If P-256 alone is broken, the combined key remains secure. |

## ECDH Public Key Validation

All P-256 public keys MUST be validated before use in ECDH operations:

1. **Curve membership:** Verify the point satisfies the P-256 curve equation (y² = x³ + ax + b mod p)
2. **Point at infinity rejection:** Reject the identity element
3. **Subgroup check:** Verify the point has order equal to the curve's prime order n (for P-256, cofactor is 1, so curve membership suffices)
4. **Format normalization:** Accept both compressed (33 bytes) and uncompressed (65 bytes) representations; normalize to uncompressed internally for consistent processing
5. **Reject invalid encodings:** Reject points with coordinates outside [0, p-1]

These checks MUST be performed in the Aegis SDK before any ECDH computation, whether the key comes from the key directory, cloud storage, or an envelope's ephemeral key field.

## ML-KEM Implementation Requirements

ML-KEM-768 (FIPS 203) being NIST-standardized does not guarantee implementation safety. The following requirements apply to the native crypto library used by the Aegis SDK:

- **Constant-time execution:** All operations involving secret key material must be constant-time (no secret-dependent branches or memory accesses)
- **FIPS validation:** The ML-KEM implementation should target FIPS 140-3 validation (CMVP) or at minimum use a library with active pursuit of validation
- **Randomness quality:** ML-KEM key generation and encapsulation require high-quality randomness from the platform's CSPRNG (SecRandomCopyBytes on iOS, SecureRandom on Android)
- **Supply-chain vetting:** The native crypto library must be audited, maintained, and have a clear provenance. Preferred libraries: Apple CryptoKit (when ML-KEM support ships), liboqs (with constant-time patches), or a vetted Rust implementation via FFI
- **Memory safety:** Key material must be wiped after use (see Memory Safety section)

## Streaming Encryption for Large Documents

For documents exceeding 4 MiB, Aegis uses a two-phase streaming construction that avoids circularity between header hashes and chunk encryption.

**Envelope type detection:** The `algorithms` field in the header includes a `mode: "single-shot" | "streaming"` indicator. Decoders check this field to determine parsing strategy before reading the payload section. Single-shot envelopes have a single nonce + ciphertext blob; streaming envelopes have a sequence of per-chunk (nonce, ciphertext) pairs followed by a signed chunk manifest.

**Phase 1 — Streaming Header and Chunk Encryption:**
1. Document payload divided into 64 KiB fixed-size chunks (final chunk variable-length)
2. Streaming header assembled (version, algorithms, recipient stanzas, sender ID, metadata, streaming mode flag) — excludes chunk hashes
3. Streaming header hash: `SHA-256(canonical_cbor(streaming_header))`
4. Each chunk encrypted: `AES-256-GCM(CEK, chunk_nonce, chunk_plaintext, aad=streaming_header_hash)`
5. Per-chunk nonce derivation: `chunk_nonce = HKDF-Expand(CEK, info="aegis-chunk-nonce" || uint64_be(chunk_index), L=12)` — deterministic derivation following the STREAM construction pattern (analogous to TLS 1.3 record layer nonce derivation per RFC 8446 §5.3)

**Phase 2 — Chunk Manifest:**
6. After all chunks encrypted, construct chunk manifest: total chunks, SHA-256 hash of each chunk's ciphertext, chunk size, total document size
7. Sign manifest: `manifest_signature = ECDSA-P256(agent_signing_key, "aegis-v1-chunk-manifest" || streaming_header_hash || SHA-256(canonical_cbor(manifest)))`
8. Signed manifest appended as separate envelope section following payload chunks

**Decryption:** Recipient verifies each chunk against manifest during streaming decryption, enabling incremental processing without buffering entire ciphertext in memory.

## Launch Scope

### MVP (v1.0)

- Flutter mobile app (iOS + Android)
- Google Drive + OneDrive
- Secure Enclave (iOS) / StrongBox or TEE Keymaster (Android) key storage
- MCP tool for Claude (client-side SDK)
- P-256 ECDH + ML-KEM-768 hybrid encryption
- Account Identity Key hierarchy with signed device key bundles (monotonic bundle_version)
- Sender authentication (ECDSA signature in envelope header)
- Basic fingerprint verification (16-char alphanumeric, derived from Account Identity Key)
- Cloud-storage-based revocation with Aegis counter-signatures (fail-closed cache)

### Phase 2

- REST API with OpenAPI spec (coordination-only, no plaintext endpoints)
- S3-compatible storage (AWS S3, MinIO)
- External hardware keys (YubiKey 5 NFC) with NFC authentication (app verifies hardware key certificate chain)
- Admin dashboard + audit logs
- Web version (WebAuthn PRF + USB YubiKey)
- Shamir's Secret Sharing recovery (K-of-N, recommended for business/enterprise)

### Phase 3

- S3-compatible storage (AWS S3, MinIO)
- SSO/SAML integration
- iCloud Drive support
- Word-based verification phrases (6-word BIP39-style)
- Nitro Enclave server-assisted recovery (enterprise)

### Future

- Key transparency log (append-only, publicly auditable)
- Automated key rotation policies
- ML-KEM-1024 (NIST Category 5) for defense/government deployments

## Memory Safety

Dart/Flutter's garbage collector does not guarantee immediate zeroing of deallocated memory. For all sensitive cryptographic material (ML-KEM private keys, shared secrets, CEKs, KEKs, decrypted plaintext):

- **Platform-native secure memory via FFI:** All crypto operations that handle key material use platform-native code (Swift/Kotlin via FFI) with explicit `memset_s`/`SecureZeroMemory` after use.
- **Minimal exposure window:** ML-KEM private key is decrypted from its SE-wrapped form only for the duration of decapsulation, then wiped immediately.
- **No Dart-side key copies:** Raw key bytes never transit through Dart `Uint8List` objects that would be subject to GC. All sensitive operations happen in native memory, with only the final ciphertext/plaintext result returned to Dart.
- **Recommended implementation: Rust core via FFI.** A single Rust crate (`aegis-crypto`) implements all cryptographic operations (ECDH, ECDSA, ML-KEM, HKDF, AES-KWP, AES-GCM, CEK generation, envelope encode/decode) and exposes a C ABI. Flutter calls this via `dart:ffi`, the agent SDK (Python/TypeScript) calls via native bindings. Benefits: single auditable codebase for both mobile platforms, memory safety guarantees from Rust's ownership model (no use-after-free, no buffer overflows), explicit `zeroize` crate for secure memory wiping, and avoids maintaining separate Swift + Kotlin crypto implementations.
- **Acknowledged limitation:** On platforms without mlock support, key material may briefly appear in swap. iOS mitigates this (encrypted swap); Android varies by device.
