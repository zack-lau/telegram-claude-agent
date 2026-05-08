# Aegis Full System Review — 2026-05-08

Sources: Qwen3.6 (infra + crypto + arch consistency), Perplexity deep research (7 queries), manual cross-analysis of all project files.

---

## PART 1 — ARCHITECTURAL DIVERGENCE (BLOCKING)

This is the most important finding in this review. **`architecture.md` (which the patent is based on) and `ADR 0002` (which the Rust API implements) describe fundamentally different products.**

| Dimension | `architecture.md` + patent | `ADR 0002` + API code |
|---|---|---|
| **Where content lives** | User's own cloud storage (Google Drive, OneDrive) — Aegis stores nothing | Aegis S3 bucket — server holds encrypted bodies |
| **Client** | Mobile app (iOS + Android) | Web recipient (browser) |
| **Private key storage** | iOS Secure Enclave / Android StrongBox | OPAQUE blob on Aegis server |
| **KEM combiner** | HKDF-SHA256 dual-PRF (NIST SP 800-56C Rev.2) | HKDF-SHA384 X-Wing IND-CCA |
| **Key wrapping** | AES-256-KWP (RFC 5649, deterministic) | AES-256-GCM (nonce required) |
| **Envelope encoding** | CBOR deterministic (RFC 8949) | JSON |
| **Sender auth** | ECDSA-P256 signature (KCI-resistant) | HPKE Auth mode (KCI-vulnerable per RFC 9180 §9.1) |
| **Key hierarchy** | Account Identity Key (P-256) + Device keys (90-day rotation) | Single keypair per recipient |

**Decision required**: Which product are we building? Or is this a planned evolution (mobile later, web-first now)?

Until this is resolved, any code written is potentially being thrown away.

### Recommendation (based on research)

**For v1 (fastest to ship, most defensible)**:
- Keep ADR 0002 server-hosted model — simpler ops, no mobile app required, easier OPAQUE integration
- Fix the cryptographic gaps: replace AES-GCM key wrapping with AES-KWP, add ECDSA sender signature overlay to fix HPKE KCI
- The patent is based on the client-side/zero-storage model — if that's a differentiation, plan a v2 mobile client

**For long-term patent alignment**:
- ADR 0002 can be the server-hosted fallback/web client
- The mobile client with Secure Enclave keys is the v2 differentiator

---

## PART 2 — CRITICAL ISSUES

### C1 — KEM Combiner Contradiction
**Source**: Qwen arch consistency [CRITICAL]

`architecture.md` uses HKDF-SHA256 dual-PRF cascade (NIST SP 800-56C Rev.2 §5.1):
```
PRK_1 = HKDF-Extract(salt=0, ikm=SS_ML-KEM)
PRK_combined = HKDF-Extract(salt=PRK_1, ikm=SS_ECDH)
K = HKDF-Expand(PRK_combined, info="aegis-v1-kek", L=32)
```

ADR 0002 D2 uses X-Wing HKDF-SHA384 with transcript binding:
```
SS_combined = HKDF-Extract(salt=prk_ecdh, ikm=prk_mlkem)
```

These produce incompatible KEKs. Implementers following different documents will produce keys that cannot decrypt each other's envelopes.

**Fix**: Pick one and remove the other. Research verdict:
- X-Wing has tighter QROM security proofs and is the CFRG/IETF direction (`draft-connolly-cfrg-xwing-kem`)
- NIST dual-PRF is FIPS-approved today but X-Wing is on track for FIPS inclusion
- For v1: use X-Wing (ADR 0002 / current code). Document FIPS compliance path for regulated customers.

**Action**: Delete the combiner description from `architecture.md` and reference ADR 0002 D2 as canonical. Add test vectors.

---

### C2 — Streaming Chunk Nonce Reuse Risk
**Source**: Qwen arch consistency [CRITICAL]

The architecture defines streaming chunk encryption:
```
chunk_nonce = HKDF-Expand(CEK, info="aegis-chunk-nonce" || uint64_be(chunk_index), L=12)
```

If an upload is interrupted and resumed from chunk 0, `chunk_index` repeats with the same CEK. GCM nonce reuse with the same key destroys confidentiality and authenticity completely — attacker can recover plaintext and forge ciphertexts.

**Fix**: Add session binding to the info parameter:
```
chunk_nonce = HKDF-Expand(CEK, info="aegis-chunk-nonce" || upload_uuid || uint64_be(chunk_index), L=12)
```
where `upload_uuid` is a fresh random UUID per upload session. Add monotonic sequence validation client-side.

**Note**: The current API `envelope.rs` uses `Aes256Gcm::generate_nonce(&mut OsRng)` (random per call) for the body nonce, which is safe for single-part encryption. The streaming issue is in the mobile client architecture — but should be documented in the ADR before implementing streaming.

---

### C3 — Terraform Plan Blocker
**Source**: Qwen infra [CRITICAL], line 25 `aegis-infra.tf`

`aws_secretsmanager_secret.app_config` is referenced in the execution role IAM policy but never defined. `terraform plan` aborts immediately.

**Fix**:
```hcl
resource "aws_secretsmanager_secret" "app_config" {
  name        = "${var.environment}/aegis/app-config"
  kms_key_id  = aws_kms_key.dynamodb.arn
}
```
Or replace with a data source if the secret is pre-created outside Terraform.

---

## PART 3 — HIGH SEVERITY ISSUES

### H1 — Cognito OAuth Config Missing
**Source**: Qwen infra [HIGH], `aegis-infra.tf:537`

The Cognito User Pool Client has `generate_secret = true` (confidential server-side client) but lacks:
- `allowed_oauth_flows = ["code"]`
- `allowed_oauth_scopes`
- `callback_urls`
- `logout_urls`

Additionally: there is no **public PKCE client** for mobile/web use. Confidential clients (with client secret) cannot be used from mobile apps or SPAs — the secret would be embedded in the client binary.

**Fix**:
```hcl
# Keep existing confidential client for server-side flows
# Add separate public PKCE client:
resource "aws_cognito_user_pool_client" "public_pkce" {
  name                          = "${var.environment}-aegis-public"
  user_pool_id                  = aws_cognito_user_pool.main.id
  generate_secret               = false
  allowed_oauth_flows           = ["code"]
  allowed_oauth_scopes          = ["openid", "email", "profile"]
  allowed_oauth_flows_user_pool_client = true
  callback_urls                 = var.callback_urls
  logout_urls                   = var.logout_urls
  explicit_auth_flows           = ["ALLOW_USER_SRP_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]
  supported_identity_providers  = ["COGNITO"]
}
```

For the existing confidential client, add the same OAuth config parameters.

---

### H2 — ECS Security Group Egress Unrestricted
**Source**: Qwen infra [HIGH], `aegis-infra.tf:193`

Current: `egress 0.0.0.0/0 protocol -1` — all outbound traffic allowed.

If an ECS container is compromised, attacker has full outbound internet access for data exfiltration, C2, scanning.

**Fix** — restrict to required endpoints only:
```hcl
# HTTPS to VPC endpoints (ECR, Secrets Manager, KMS, CloudWatch, S3)
egress {
  from_port   = 443
  to_port     = 443
  protocol    = "tcp"
  cidr_blocks = [aws_vpc.main.cidr_block]
}
# HTTPS to Cognito (external)
egress {
  from_port   = 443
  to_port     = 443
  protocol    = "tcp"
  cidr_blocks = ["0.0.0.0/0"]
  description = "Cognito + SES (scoped by WAF)"
}
```

Better: add VPC interface endpoints for all AWS services (see Missing Infra section), then lock egress to VPC CIDR only.

---

### H3 — Fail-Closed Revocation Cache = Total Outage
**Source**: Qwen arch consistency [HIGH]

The architecture specifies that agents MUST NOT encrypt if the revocation service is unreachable for >1 hour (fail-closed). If the timestamp service or S3 revocation folder goes down, ALL AI agent encryption stops.

**Fix**: Implement circuit breaker with configurable grace period:
```
- Normal: check revocation cache (TTL 1h)
- Cache stale + service unreachable: enter grace period (configurable, default 4h)
- During grace period: encrypt with warning logged, alert ops
- Grace period expired: fail-closed
```
Deploy revocation service with multi-region redundancy. Cache "last known good" state.

---

### H4 — HPKE Auth KCI Vulnerability
**Source**: Perplexity research + RFC 9180 §9.1

HPKE Auth mode (used in ADR 0002 D4 for sender authentication) has a Key Compromise Impersonation vulnerability: if a **receiver's** private key is compromised, an adversary can forge messages appearing to come from any sender. This is documented in RFC 9180 §9.1.

The original `architecture.md` uses ECDSA-P256 signing which is KCI-resistant (attacker needs the **sender's** key to forge sender auth).

**Fix**: Add a separate ECDSA-P256 or Ed25519 signature over the envelope header as a sender-authenticated outer layer. Keep HPKE for encryption, add explicit signature for sender binding:
```
EnvelopeHeader.sender_signature = Ed25519.sign(sk_sender, SHA-384(canonical_header_bytes))
```
This is the approach in the original architecture and is correct.

**Alternative**: Use HPKE Auth for encryption but document the KCI limitation; require out-of-band sender key verification for high-security use cases.

---

### H5 — Bundle Expiry Relies on Device Clock
**Source**: Qwen arch consistency [HIGH]

Key bundle validation checks `expires_at` against device clock. NTP failure, timezone misconfiguration, or manual clock adjustment can cause valid bundles to be rejected OR expired bundles to be accepted.

**Fix**: Require NTP synchronization for bundle validation. Include a server-signed timestamp in the bundle:
```json
{
  "bundle": { ... },
  "server_timestamp": { "ts_ms": 1234567890000, "signature": "<Aegis CA sig>" }
}
```
Client validates: `|device_clock - server_timestamp| < tolerance_window (e.g. 5 min)`. If drift too large, warn user and refuse to use bundle.

---

### H6 — OAuth Refresh Token Compromise Vector
**Source**: Qwen arch consistency [HIGH]

The ADR stores OAuth refresh tokens encrypted with AES-256-GCM, key in AWS KMS. A server breach or insider with KMS access can decrypt these and write arbitrary files to recipients' cloud storage.

**Fix**:
1. Token rotation on every use (ADR 0001 already specifies this — verify implementation)
2. Strict IAM: KMS decrypt permission should be scoped to the OAuth token handling Lambda/service only, not the general ECS task role
3. Add upload integrity checks: sign upload requests with the sender's key so recipients can detect unauthorized writes
4. Alert on token reuse (ADR 0001 epoch invalidation)

---

## PART 4 — MEDIUM SEVERITY ISSUES

### M1 — DynamoDB GSI ARN Format Incorrect
**Source**: Qwen infra [MEDIUM], `aegis-infra.tf:57`

Current IAM policy likely uses `${aws_dynamodb_table.api_keys.arn}:index/owner_id-index` (colon) instead of the correct slash format.

**Fix**:
```hcl
"${aws_dynamodb_table.api_keys.arn}/index/owner_id-index"
```

---

### M2 — KMS Policy Resource = "*"
**Source**: Qwen infra [MEDIUM], `aegis-infra.tf:367`

ECS task role has `Resource = "*"` in KMS policy. If new KMS keys are added, the task could accidentally decrypt them.

**Fix**:
```hcl
Resource = [aws_kms_key.oauth_tokens.arn]
```

---

### M3 — NAT Gateway Single AZ
**Source**: Qwen infra [MEDIUM], `aegis-infra.tf:135`

Single NAT Gateway = AZ failure causes complete outbound connectivity loss for private subnets.

**Fix**: Deploy NAT Gateways in each AZ with per-AZ route tables. Or accept this risk for dev/staging, document for prod.

---

### M4 — OPAQUE Misapplied
**Source**: Qwen arch consistency [MEDIUM]

OPAQUE (RFC 9807) is a PAKE protocol designed for password-authenticated key exchange. Using it to derive `k_wrap` for encrypting private key blobs is non-standard — OPAQUE was designed for server authentication, not client-side key wrapping.

**Research finding**: `opaque-ke v4.1.0-pre.2` is the only Rust crate and it's pre-release. The RFC itself is from 2024 and the ecosystem is immature.

**Options**:
1. **Argon2id + HKDF** (simpler, battle-tested): `k_wrap = HKDF-SHA384(Argon2id(password, salt, params), info="aegis-v1 private key wrap")`
2. **OPAQUE properly**: Use it for the login flow (server learns nothing about password), then derive `k_wrap` from the OPAQUE export key. This is actually the correct OPAQUE usage but requires the full OPAQUE handshake on every login.

**Recommendation**: Use OPAQUE properly (option 2) for its intended purpose — export key for key wrapping is exactly what the RFC export key is designed for. Unblock by using `opaque-ke v4.1.0-pre.2` with a version pin and plan to stabilize on GA release.

---

### M5 — Missing Error Propagation in Deduplication
**Source**: Qwen arch consistency [MEDIUM]

The app silently skips already-seen `doc_id` values. If the local SQLite dedup check fails or the DB is corrupted, valid envelopes could be silently dropped. Decryption failures lack structured error handling.

**Fix**: 
- Log dedup skips with structured context (envelope ID, recipient, reason)
- Make dedup check + DB write atomic (transaction)
- Propagate decryption errors with error codes, not silent drops

---

### M6 — AES-GCM vs AES-KWP for Key Wrapping
**Source**: Architecture.md vs ADR 0002 analysis + Perplexity research

`architecture.md` correctly uses AES-256-KWP (RFC 5649, NIST SP 800-38F) for key wrapping. ADR 0002 uses AES-256-GCM with a random nonce.

**Problem with AES-GCM for key wrapping**: If the same key is used to wrap multiple keys (or the same key multiple times in edge cases), random 96-bit nonce collision probability is non-trivial at scale. NIST recommends retiring AES-GCM keys after 2^32 random nonce uses — a busy multi-tenant system could hit this.

**AES-KWP advantages** (NIST SP 800-38F):
- Deterministic — no nonce needed, no nonce reuse risk
- Specifically designed for wrapping key material
- No additional per-wrap random generation needed

**Fix**: Replace `wrap_key()` in `envelope.rs` with AES-KWP:
```rust
use aes_kw::Kek;
let kek = Kek::<Aes256>::new(&wrap_key_bytes.into());
let wrapped = kek.wrap_with_padding_vec(k_content)?;
```
Use crate: `aes-kw = "0.2"` with feature `alloc`.

This is a breaking change to the wire format — do it now before any envelopes are in production.

---

## PART 5 — LOW SEVERITY / FUTURE CONCERNS

### L1 — unwrap_key Timing Side Channel
**Source**: Qwen crypto [LOW]

`aes-gcm` in Rust uses the `aes` crate which uses AES-NI hardware intrinsics — timing is inherently constant for the cipher. However, the length check (`wrapped.len() != WRAPPED_KEY_LEN`) short-circuits before AEAD verification in a way that leaks whether the input was the right length.

**Practical risk**: Very low. An attacker needs many oracle queries. Not exploitable for key recovery in standard threat models.

**Fix**: Ensure the length check error message is identical to the AEAD failure message. Consider `subtle::ConstantTimeEq` for the length comparison if operating under extreme adversarial conditions.

---

### L2 — open() Doesn't Bind recipient_id to Key
**Source**: Qwen crypto [LOW]

`open()` in `envelope.rs` accepts `recipient_id` as a parameter but doesn't verify it matches the identity bound to `sk`. If the API layer has a bug and passes wrong IDs, a recipient could open an envelope it shouldn't (if it also happens to be listed in the recipients array).

**Fix**: The API layer (`routes/deliveries.rs`) must extract `recipient_id` from the verified JWT and pass it to `open()`. This should already be the case — verify in the route handler. Could also bind recipient identity to the KEM keypair during registration.

---

### L3 — secretsmanager GetSecretValue Without Resource Restriction
**Source**: Qwen infra [LOW], `aegis-infra.tf:25`

Execution role has broad `secretsmanager:GetSecretValue`. Should be scoped to specific secret ARNs.

---

### L4 — FFI Boundary Memory Safety (Mobile)
**Source**: Qwen arch consistency [LOW]

Dart/Flutter GC doesn't guarantee immediate zeroing. The Rust FFI boundary with `zeroize` may not protect against Dart GC holding references.

**Fix**: Isolate sensitive FFI calls. Use `SecureMemory` abstractions. Add CI memory leak detection.

---

## PART 6 — MISSING INFRASTRUCTURE

These are gaps not in any Terraform file today.

| Missing | Impact | Fix |
|---|---|---|
| S3 delivery body bucket | POST /deliveries can't store encrypted bodies | Add `aws_s3_bucket.delivery_bodies` with SSE-KMS, versioning, lifecycle rules |
| Deliveries DynamoDB table | API route stubs will fail | Add `aws_dynamodb_table.deliveries` |
| S3 IAM for ECS task | Task role can't write/read delivery bodies | Add `s3:PutObject`, `s3:GetObject`, `s3:DeleteObject` for the delivery bucket |
| ECR API + DKR VPC endpoints | Fargate can't pull images in private subnet (uses NAT, expensive, unreliable) | Add `aws_vpc_endpoint` interface for `ecr.api`, `ecr.dkr` |
| Secrets Manager VPC endpoint | Fargate can't fetch secrets without NAT | Add interface endpoint for `secretsmanager` |
| KMS VPC endpoint | KMS calls go via NAT | Add interface endpoint for `kms` |
| CloudWatch Logs VPC endpoint | Logs go via NAT | Add interface endpoint for `logs` |
| SES integration | No email notifications (new device, token reuse, re-fetch alerts) | Add SES sending identity + IAM + route handler |

---

## PART 7 — MISSING API IMPLEMENTATIONS

| Missing | File | Notes |
|---|---|---|
| OPAQUE registration/login | `crypto/opaque.rs` | Returns `Err("pending")` — blocks key storage |
| Aegis CA signing via KMS | `routes/keys.rs` | Currently writes zero bytes as signature; should use KMS Ed25519 (now GA) |
| Session epoch invalidation | `db/sessions.rs` | ADR 0001 specifies epoch counter; table/counter not implemented |
| Audit log writes | all routes | IAM has append-only audit_logs policy; routes never write to it |
| POST /deliveries full seal | `routes/deliveries.rs` | Stub — needs: fetch sender key bundle, build recipient list, call `seal()`, write header to DDB + body to S3 |
| Burn-after-read S3 delete | `routes/deliveries.rs` GET | After successful decrypt, delete body from S3 if `burn_after_read = true` |
| Agent registration flow | `routes/` | No route for agents to register themselves and get API keys |
| Key rotation flow | `routes/keys.rs` | POST /me/keys creates; no rotation with transition period |
| JWKS cache refresh | `crypto/jwt.rs` | `JwtValidator` refreshes on kid miss — good. Verify TTL-based proactive refresh |

---

## PART 8 — OPEN QUESTIONS REQUIRING DECISIONS

These require explicit founder/team decisions, not just coding.

1. **Product model**: Web-first (ADR 0002, server-hosted) or mobile-first (architecture.md, client-side)? Or both in phases?

2. **FIPS compliance**: Regulated customers (healthcare, finance, gov) will ask. X-Wing is not FIPS-approved today. NIST SP 800-56C dual-PRF is. Is this a near-term requirement? If yes, use the architecture.md combiner.

3. **OPAQUE vs Argon2id for key wrapping**: OPAQUE is the right tool but the Rust ecosystem is pre-release. Argon2id is battle-tested. Timeline to GA OPAQUE?

4. **Streaming encryption**: The architecture defines streaming for large docs. Current API does single-part AES-GCM. When is streaming needed? Need to design nonce scheme before that work starts.

5. **Multi-device sync**: architecture.md describes Account Identity Key with per-device subkeys. ADR 0002 has single keypair. How do recipients use Aegis from multiple devices?

6. **Key recovery**: What happens when a recipient loses their private key? Current design has no recovery path. OPAQUE with server-side encrypted blob is the recovery path — but if OPAQUE server is down or keys are corrupted, all messages are permanently lost.

7. **Agent identity**: How do AI agents prove they're authorized to encrypt on behalf of an organization? The API has API key auth but no agent attestation or capability scoping.

8. **Audit log format**: What goes in the audit log? Who can read it? The IAM has append-only enforcement but the schema isn't defined.

9. **Pricing model alignment**: `pricing.md` describes Google Drive integration and mobile-first features. This doesn't match ADR 0002's server-hosted model. Need to reconcile product → technical architecture → pricing.

---

## PART 9 — PRIORITY ACTION LIST

### Do immediately (blockers)
1. **Decide on product model** (web-first vs mobile-first) — everything else depends on this
2. **Fix Terraform plan blocker** — define `aws_secretsmanager_secret.app_config` (C3)
3. **Unify KEM combiner** — one canonical spec, delete conflicting description (C1)
4. **Fix streaming nonce** — add upload UUID to chunk nonce info before implementing streaming (C2)

### Do before first deployment
5. **Add ECDSA signature overlay** for sender auth (fix HPKE KCI, H4)
6. **Fix Cognito OAuth config** + add public PKCE client (H1)
7. **Restrict ECS egress** security group (H2)
8. **Add VPC endpoints** for ECR, Secrets Manager, KMS, CloudWatch Logs
9. **Add missing Terraform resources**: S3 delivery bucket, deliveries DDB table
10. **Migrate Aegis CA key to KMS Ed25519** (now GA)
11. **Replace AES-GCM key wrapping with AES-KWP** — breaking wire format change, do now (M6)
12. **Fix DynamoDB GSI ARN format** (M1)
13. **Scope KMS policy Resource** to specific ARN (M2)

### Do before public beta
14. **Implement OPAQUE** (or Argon2id interim) for key storage
15. **Implement session epoch invalidation**
16. **Write POST /deliveries** full sealing pipeline
17. **Add burn-after-read** S3 delete
18. **Add audit log writes** to all mutating routes
19. **Add SES notifications** for security events
20. **Design + implement bundle clock verification** (H5)
21. **Design multi-device sync** and key recovery

---

## Summary

The code and crypto are fundamentally sound — the hybrid KEM combiner, AAD binding, and zeroization are well-implemented. The blocking issues are architectural and operational: the product model divergence between `architecture.md` and `ADR 0002` needs to be resolved before more code is written, the Terraform has a hard blocker, and several high-severity security gaps (HPKE KCI, GCM nonce risk for key wrapping, fail-closed revocation) need design decisions before deployment.

The three most important questions to answer today:
1. Which product model is v1? (determines which ADR is canonical)
2. FIPS requirement near-term? (determines which KEM combiner to keep)
3. OPAQUE or Argon2id for key storage? (unblocks the entire key storage layer)
