# ADR 0001: Multi-Session Authentication Architecture

**Status:** Accepted  
**Date:** 2026-05-08  
**Deciders:** Zack  

---

## Context

Aegis delivers encrypted work product from AI agents to human recipients. Recipients authenticate to decrypt and access their deliveries. The core security property is: **whoever holds a valid session for `recipient_id` can access all encrypted deliveries addressed to that recipient.** This makes account takeover higher-impact than a typical SaaS — it's not profile data at risk, it's sensitive AI-generated work product.

Recipients are primarily humans accessing Aegis from multiple devices (phone, laptop, work PC). AI agent clients use API keys (`api_keys` table), not OAuth.

---

## Decision

**Allow multiple concurrent sessions per recipient, bounded by a session cap, with the following security controls mandatory before GA.**

Single-session was rejected because:
- Breaks phone + laptop simultaneous use (common for knowledge workers)
- "Kicked out" UX is hostile for an episodic-use product (check deliveries, close tab)
- Enterprise recipients expect SSO with normal multi-device behaviour

---

## Identity Provider

**AWS Cognito User Pool** handles all credential operations:
- Email + password (personal clients) via SRP auth flow
- SAML/OIDC federation (enterprise SSO: Google Workspace, Entra ID, Okta)
- MFA: TOTP optional at launch, REQUIRED for enterprise tier at GA
- Magic link: custom auth Lambda trigger (phase 2)

Aegis owns only **session metadata** in DynamoDB. Cognito owns credentials, JWT issuance, and token revocation endpoint (RFC 7009).

Cognito `sub` (stable UUID per user per pool) = `recipient_id` across all tables.

---

## Token Model

| Token | Lifetime | Storage | Notes |
|---|---|---|---|
| Access token | 30 minutes | Memory only, never persisted | JWT, validated stateless against Cognito JWKS |
| Refresh token | 30 days | HttpOnly Secure cookie (web) / Secure Enclave (mobile) | Opaque, single-use, rotated on every use |

**Never store tokens in localStorage or sessionStorage.** XSS steals them trivially. Use HttpOnly cookies for web clients and platform secure storage for mobile.

Access tokens are validated stateless — verify JWT signature against `{cognito_jwks_uri}` output, check `exp`, check `iss` matches `{cognito_issuer_url}`. No DynamoDB hit on every request.

---

## Session Metadata Schema (DynamoDB `oauth_tokens`)

```
hash_key  = recipient_id   (Cognito sub)
range_key = token_id       (UUID v4, issued at session creation)

token_value_hash    STRING   SHA-256 of the refresh token value (GSI hash key)
token_type          STRING   "access" | "refresh"
token_family_id     STRING   UUID linking all rotations of this session (for family revocation)
version             NUMBER   Monotonically incrementing — used for conditional writes on rotation
auth_provider       STRING   "cognito" | "google" | "entra" | "okta" | "magic_link"
expires_at          NUMBER   Unix epoch — DynamoDB TTL attribute (auto-delete)
created_at          STRING   ISO 8601
last_used_at        STRING   ISO 8601 — updated on each successful use
device_hint         STRING   Truncated user-agent + platform (e.g., "Safari/macOS")
ip_at_creation      STRING   IPv4/IPv6 of session creation request
session_epoch       NUMBER   Copied from recipient's current epoch at session creation
```

**GSI:** `token_value_hash-index` (hash=`token_value_hash`) — used to look up a session by bearer token value on every refresh request. Projection: KEYS_ONLY.

---

## Mandatory Security Controls

### 1. Atomic Refresh Token Rotation (Critical — Qwen finding)

Concurrent refresh requests on the same token (multiple tabs, background sync, race on mobile) cause either silent auth failures or token reuse if not handled atomically.

**Implementation:**
- On refresh: DynamoDB `UpdateItem` with `ConditionExpression = "version = :current_version"`
- If `ConditionalCheckFailedException` → another request already rotated this token
- Respond with `400 token_already_rotated`, client retries with the new token from the racing request
- Token family ID is carried forward to the new token row

**Never** allow two valid tokens to exist for the same session simultaneously. The version counter enforces this.

Reference: [Nango on concurrent OAuth token refreshes](https://nango.dev/blog/concurrency-with-oauth-token-refreshes/), OAuth 2.0 BCP.

---

### 2. Refresh Token Reuse Detection → Full Recipient Revocation (Critical — Opus + Qwen)

If a previously-rotated refresh token is presented again, this is proof of theft. One copy was used by the attacker, one by the legitimate user, and now the stale copy is being replayed.

**Implementation:**
- On refresh: query `token_value_hash-index` for the presented token hash
- If row exists AND `version > 0` AND the token is already rotated (check against current `token_id` in session) → **reuse detected**
- Action: delete ALL rows in `oauth_tokens` where `recipient_id = <victim>` (full recipient wipe)
- Call `cognito-idp:AdminUserGlobalSignOut` to invalidate all Cognito sessions
- Emit `SECURITY_TOKEN_REUSE` event to `audit_logs`
- Return `401` to both the attacker and the next legitimate refresh attempt — forces full re-auth
- Send out-of-band email to recipient: "suspicious activity detected, all sessions ended"

This is the single most effective control against stolen refresh tokens. Cheap to implement on existing schema.

Reference: Auth0 refresh token rotation docs, Obsidian Security OAuth best practices.

---

### 3. Epoch Invalidation on Password Change (Critical — Opus)

Password change is the user's kill switch. It must actually work.

**Implementation:**
- Maintain `session_epoch` counter on the recipient record (separate table or attribute)
- Embed current epoch as a custom Cognito claim or store in DDB session row at creation
- On password change:
  1. Increment `session_epoch` for `recipient_id`
  2. Delete ALL rows in `oauth_tokens` where `recipient_id = <user>` — atomic batch delete
  3. Call `cognito-idp:AdminUserGlobalSignOut`
- On access token validation: check `session_epoch` claim matches current epoch (one DDB read, cacheable for the token's 30-min lifetime)
- Any session created before the epoch increment is dead instantly, regardless of token expiry

Do NOT rely on `expires_at` alone. Epoch invalidation makes password change an actual kill switch.

---

### 4. Session Cap + Eviction Policy (Opus)

**Cap:** Maximum 5 concurrent sessions per `recipient_id`.

**Eviction on new login (cap reached):**
- Delete the oldest row by `created_at` (FIFO)
- Exception: if a "trusted device" flag exists on a session, it cannot be evicted by cap pressure

**Trusted device:**
- After successful MFA on a device, mark that `token_id` as `trusted = true`
- One trusted-device slot reserved per recipient — cannot be FIFO-evicted
- Guarantees the user always has a way back in even if an attacker hits the cap

**Session cap DoS mitigation:** new sessions from a previously-unseen device require MFA before consuming a slot. This prevents an attacker (who has credentials but not MFA) from spamming sessions to lock out the user.

---

### 5. User-Visible Active Sessions + Per-Delivery Access Log (Opus)

Detection crowdsourced to the recipient — the only person who knows what's legitimate.

**Active sessions endpoint:** `GET /me/sessions`  
Returns: `token_id` (redacted), `device_hint`, `ip_at_creation`, `last_used_at`, `auth_provider`, `created_at`  
Allows user to revoke individual sessions: `DELETE /me/sessions/{token_id}`

**Per-delivery access log:** every decrypt/fetch event logged in `audit_logs` with `token_id`, `device_hint`, `ip`. Surfaced to user as "this delivery was accessed from: [list]".

**New device email:** on session creation from a previously-unseen `device_hint`, send an out-of-band email ("New sign-in from Safari/macOS — not you? Revoke all sessions here").

---

### 6. Cognito Advanced Security: AUDIT → ENFORCED (Opus)

Currently in AUDIT mode (logs anomalies, doesn't block). Flip to ENFORCED before GA:
- Impossible travel → step-up MFA required (not block, to avoid breaking VPN users)
- New device + risk score > threshold → step-up MFA
- Credential stuffing pattern → temporary account lockout

Cost: occasional MFA prompts for legitimate travellers. Acceptable for a product delivering sensitive encrypted work product.

---

## Phase 2: Device Binding via DPoP (RFC 9449)

DPoP (Demonstrating Proof-of-Possession) cryptographically binds tokens to the client's device key. A stolen token cannot be used from a different device because the attacker doesn't have the private key.

**How it works:**
- Client generates an asymmetric key pair on first launch (EC P-256)
- Web: Web Crypto API with non-exportable key, stored in IndexedDB
- Mobile: iOS Secure Enclave / Android Keystore (hardware-backed)
- Every token request includes a DPoP proof JWT signed with the private key
- Server verifies proof, binds token to public key thumbprint (`jkt` claim per RFC 9449)
- Stolen token without the private key = unusable

**Aegis-specific impact:** defeats token theft (#2 threat) at the crypto layer. Even malware that exfiltrates the refresh token can't use it without the device key.

**Phase 2 because:** adds meaningful client SDK complexity. Phase 1 controls (reuse detection, epoch invalidation, session cap) address the same threats at lower implementation cost and must ship first.

Reference: RFC 9449, Auth0 DPoP guide, FIDO Alliance DBSC+DPoP whitepaper.

---

## Phase 2: Aegis-Side Passkey (Independent of Cognito/IdP)

For enterprise clients, SSO IdP compromise (Okta/Entra breach, helpdesk social engineering) is the worst non-cryptographic failure mode. A SAML assertion looks genuine to Cognito even if the IdP was compromised.

**Mitigation:** Aegis enrolls a passkey (WebAuthn) per recipient on first login, stored Aegis-side independent of the IdP. New device login requires both a valid Cognito session AND a passkey assertion. The IdP helpdesk cannot enroll an Aegis passkey.

**Additionally:** 24-hour cooldown + out-of-band email before a new device can decrypt deliveries. Even after a helpdesk-assisted IdP reset, the attacker must wait 24h and the victim gets an email.

---

## Delivery Replay Protection

Authenticated attacker re-fetches decrypted deliveries. Encryption is moot once decryption keys are accessible to an authenticated session.

**Controls:**
- Mark `decrypted_at` on first successful delivery fetch
- Subsequent fetches from a different `token_id` emit a `SECURITY_DELIVERY_REFETCH` audit log event and trigger an out-of-band email notification
- Rate-limit: max 3 fetches per delivery per session per hour
- Delivery nonces + idempotency keys on the decryption API to prevent request replay at the transport layer

---

## What Cognito Handles vs. What Aegis Handles

| Concern | Owner |
|---|---|
| Credential storage + password hashing | Cognito |
| SSO federation (SAML/OIDC) | Cognito |
| JWT issuance + signing | Cognito |
| MFA (TOTP) | Cognito |
| Brute force / credential stuffing protection | Cognito Advanced Security |
| Risk-based adaptive auth (impossible travel) | Cognito Advanced Security |
| RFC 7009 token revocation endpoint | Cognito |
| Session metadata (device, IP, last_used) | Aegis (DynamoDB) |
| Session cap enforcement | Aegis |
| Refresh token reuse detection | Aegis |
| Epoch invalidation on password change | Aegis |
| Per-delivery access log | Aegis (audit_logs) |
| Active sessions UI | Aegis |
| DPoP proof verification (phase 2) | Aegis |
| Passkey enrollment (phase 2) | Aegis |

---

## Implementation Checklist (Phase 1 — Required Before GA)

- [ ] DynamoDB conditional writes on refresh token rotation (version counter)
- [ ] `ConditionalCheckFailedException` → 400, client retries with racing token
- [ ] Refresh token reuse detection → full recipient session wipe + AdminUserGlobalSignOut + audit log + email
- [ ] `session_epoch` counter on recipient record
- [ ] Password change → increment epoch + batch-delete all oauth_tokens + AdminUserGlobalSignOut
- [ ] Access token validation checks session_epoch
- [ ] Session cap (max 5) with FIFO eviction + trusted-device reserved slot
- [ ] New-device step-up MFA before session slot consumed
- [ ] `GET /me/sessions` and `DELETE /me/sessions/{token_id}` endpoints
- [ ] Per-delivery access log in audit_logs
- [ ] New-device email notification
- [ ] Cognito Advanced Security → ENFORCED
- [ ] `decrypted_at` tracking on deliveries + re-fetch email notification
- [ ] Refresh tokens in HttpOnly Secure cookies (web client)

## Implementation Checklist (Phase 2 — Post-Launch)

- [ ] DPoP (RFC 9449) implementation — client SDK + server proof verification
- [ ] Aegis-side passkey enrollment (WebAuthn, independent of IdP)
- [ ] 24h new-device cooldown before decryption allowed
- [ ] Magic link auth flow (Cognito custom auth Lambda)
- [ ] Custom auth domain (auth.aegis.prosteclabs.com — requires us-east-1 ACM cert)
- [ ] SES integration for transactional emails (replace Cognito default)
- [ ] Per-tenant IdP certificate pinning for enterprise SSO

---

## References

- RFC 9449 — OAuth 2.0 Demonstrating Proof-of-Possession (DPoP)
- RFC 7009 — OAuth 2.0 Token Revocation
- RFC 6819 — OAuth 2.0 Threat Model and Security Considerations
- OAuth 2.1 draft — mandatory PKCE + refresh token rotation for public clients
- Auth0: Refresh Token Rotation docs
- FIDO Alliance: DBSC + DPoP as complementary technologies
- Nango: Concurrency with OAuth token refreshes
- Obsidian Security: OAuth refresh token best practices
