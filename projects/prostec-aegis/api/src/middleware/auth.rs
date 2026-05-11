// Bearer token authentication middleware.
// Validates Cognito JWT access tokens; injects AuthenticatedUser into request extensions.
//
// Session epoch check is NOT done here — it's done in route handlers that need it,
// so the middleware stays cheap on every request.

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientType {
    Web,
    Mobile,
    Agent,
}

/// Injected into request extensions by the auth middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// Cognito sub (stable UUID per user per pool) = recipient_id.
    pub recipient_id: Uuid,
    pub raw_sub: String,
    pub groups: Vec<String>,
    pub client_type: ClientType,
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&req)?;
    let claims = state
        .jwt()
        .validate(token)
        .await
        .map_err(|_| ApiError::Unauthorized)?;

    let recipient_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    // Server-determined ClientType per ADR 0004: derive from the JWT's `client_id`
    // claim (one of the configured per-platform Cognito app clients), with the
    // `x-aegis-client-type` header used only as a back-compat fallback during
    // migration. The header alone is spoofable; the JWT claim is signed by Cognito.
    let client_type = classify_client(claims.client_id.as_deref().unwrap_or(""), state.cfg())
        .unwrap_or_else(|| extract_client_type(&req));

    // Single recipient_settings lookup (strongly consistent). Enforces BOTH:
    //   1. FIPS strict mode for web clients (Fix C2/H9 — fail closed on any AWS error).
    //   2. Session epoch kill-switch: reject access tokens issued before the most recent
    //      `increment_session_epoch` (Codex Round 3 critical finding). This is what makes
    //      password change / account-compromise revocation actually invalidate stolen tokens.
    use crate::db::recipient_settings::RecipientSettingsStore;
    let settings_store = RecipientSettingsStore::new(
        state.ddb(),
        &state.cfg().dynamodb_table_prefix,
    );
    let settings = settings_store
        .get(recipient_id)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("recipient_settings unavailable: {}", e)))?;

    if client_type == ClientType::Web && settings.fips_strict {
        return Err(ApiError::Forbidden);
    }

    // Epoch enforcement: token was issued at `claims.iat` (unix seconds). If a recent
    // `increment_session_epoch` happened AFTER the token was issued, the token is revoked.
    // Bound the cast: a u64 iat > i64::MAX would wrap negative and bypass the check (M-7).
    let iat_i64 = i64::try_from(claims.iat).unwrap_or(i64::MAX);
    if settings.last_epoch_increment_at > 0 && iat_i64 < settings.last_epoch_increment_at {
        return Err(ApiError::Unauthorized);
    }

    let user = AuthenticatedUser {
        recipient_id,
        raw_sub: claims.sub,
        groups: claims.groups,
        client_type,
    };

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

fn extract_client_type(_req: &Request) -> ClientType {
    // Header fallback cannot grant Mobile or Agent — those require the Cognito-signed
    // JWT `client_id` claim (C-2). Returning Web is the fail-safe default and ensures
    // fips_strict enforcement is never bypassed via an unsigned header.
    ClientType::Web
}

/// Map the JWT `client_id` claim to a `ClientType` using per-platform Cognito app
/// client IDs from config (ADR 0004). Returns `None` if no per-platform app client
/// is configured OR the claim doesn't match any of them — caller falls back to the
/// (legacy) header-based classification.
fn classify_client(client_id_claim: &str, cfg: &crate::config::Config) -> Option<ClientType> {
    if client_id_claim.is_empty() {
        return None;
    }
    if !cfg.cognito_client_id_mobile.is_empty() && client_id_claim == cfg.cognito_client_id_mobile {
        return Some(ClientType::Mobile);
    }
    if !cfg.cognito_client_id_web.is_empty() && client_id_claim == cfg.cognito_client_id_web {
        return Some(ClientType::Web);
    }
    if !cfg.cognito_client_id_agent.is_empty() && client_id_claim == cfg.cognito_client_id_agent {
        return Some(ClientType::Agent);
    }
    None
}

fn extract_bearer_token(req: &Request) -> ApiResult<&str> {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;

    header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)
}

/// API-key authentication for AI agents.
///
/// Wire format of the header: `x-api-key: aegis_<key_id>_<secret>` where
///   - `key_id` is the public identifier stored as the DynamoDB hash key
///   - `secret` is the high-entropy half; server compares SHA-256(secret) to `secret_hash`
///     using a constant-time equality test
///
/// On success, injects `AgentIdentity` into request extensions.
pub async fn require_api_key(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    use crate::db::api_keys::ApiKeyStore;
    use crate::models::api_key::AgentIdentity;
    use aws_lc_rs::{constant_time, digest};

    let raw = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;

    // Parse: aegis_<key_id>_<secret>
    let stripped = raw.strip_prefix("aegis_").ok_or(ApiError::Unauthorized)?;
    let (key_id, secret) = stripped.split_once('_').ok_or(ApiError::Unauthorized)?;
    if key_id.is_empty() || secret.is_empty() {
        return Err(ApiError::Unauthorized);
    }

    let store = ApiKeyStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let api_key = store
        .get(key_id)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("api_keys lookup: {}", e)))?
        .ok_or(ApiError::Unauthorized)?;

    if !api_key.active || api_key.is_expired() {
        return Err(ApiError::Unauthorized);
    }

    // Compute SHA-256(secret) hex and compare in constant time against stored hash.
    let computed_hash: String = digest::digest(&digest::SHA256, secret.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    if api_key.secret_hash.len() != computed_hash.len()
        || constant_time::verify_slices_are_equal(
            api_key.secret_hash.as_bytes(),
            computed_hash.as_bytes(),
        )
        .is_err()
    {
        return Err(ApiError::Unauthorized);
    }

    // Optional IP allowlist enforcement.
    //
    // X-Forwarded-For is a chain of IPs that PROXIES APPEND TO. The leftmost entry
    // is whatever the original client SAID — attacker-controlled, never trustable.
    // The trustable entry is the LAST one, which the immediate trusted proxy
    // (our ALB) injects. ALB always appends the actual TCP peer to XFF.
    // (Qwen Round 7 auth HIGH on IP spoofing.)
    if !api_key.allowed_ips.is_empty() {
        let peer_ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit(',').next())
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        if peer_ip.is_empty() || !api_key.allowed_ips.iter().any(|allow| allow == &peer_ip) {
            tracing::warn!(key_id = %api_key.key_id, peer_ip = %peer_ip, "api key rejected by IP allowlist");
            return Err(ApiError::Forbidden);
        }
    }

    // Best-effort touch; signature is `async fn` (no Result) so it cannot accidentally
    // bubble up via `?` and fail the auth path on a transient DDB blip.
    store.touch_last_used(&api_key.key_id).await;

    let identity = AgentIdentity {
        key_id: api_key.key_id,
        owner_id: api_key.owner_id,
        role: api_key.role,
        allowed_recipients: api_key.allowed_recipients,
    };
    req.extensions_mut().insert(identity);
    Ok(next.run(req).await)
}
