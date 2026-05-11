use aws_lc_rs::digest::{Context as DigestContext, SHA256};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::db::key_directory::KeyDirectoryStore;
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthenticatedUser;
use crate::models::key_directory::{KeyBundleResponse, KeyDirectoryRecord, RegisterKeyBundleRequest};
use crate::state::AppState;

/// GET /keys/{recipient_id} — anyone can fetch a recipient's signed key bundle.
/// Senders verify the bundle against the pinned AIK fingerprint (out-of-band).
pub async fn get_key_bundle(
    State(state): State<AppState>,
    Path(recipient_id): Path<Uuid>,
) -> ApiResult<Json<KeyBundleResponse>> {
    let store = KeyDirectoryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let record = store.get(recipient_id).await.map_err(ApiError::Internal)?;
    let record = record.ok_or(ApiError::NotFound)?;

    if record.bundle_expiry < chrono::Utc::now() {
        return Err(ApiError::NotFound);
    }

    let signed_bundle = record
        .parse_signed_bundle()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("corrupt signed_bundle_json: {}", e)))?;

    Ok(Json(KeyBundleResponse {
        recipient_id,
        aik_fingerprint_hex: record.aik_fingerprint_hex,
        bundle_version: record.bundle_version,
        bundle_expiry: record.bundle_expiry,
        signed_bundle,
    }))
}

/// POST /me/keys — recipient registers or rotates their AIK-signed key bundle.
///
/// Server responsibility:
///   1. Verify the AIK signature on the bundle and on each device entry.
///   2. Verify the bundle's `recipient_id` matches the authenticated user's Cognito sub.
///   3. Conditional-write into DynamoDB so `bundle_version` is monotonically increasing
///      (the put() helper enforces this via a ConditionExpression).
///   4. Persist the encrypted private key blob (OPAQUE-wrapped) alongside the public bundle.
///
/// The Aegis service does NOT sign — the trust root is the recipient's AIK, not Aegis.
pub async fn register_key_bundle(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(req): Json<RegisterKeyBundleRequest>,
) -> ApiResult<(StatusCode, Json<KeyBundleResponse>)> {
    if req.signed_bundle.bundle.recipient_id != user.recipient_id {
        return Err(ApiError::BadRequest(
            "signed_bundle.recipient_id does not match authenticated user".to_owned(),
        ));
    }

    req.signed_bundle
        .verify()
        .map_err(|e| ApiError::BadRequest(format!("bundle verification failed: {}", e)))?;

    let aik_fingerprint_hex = aik_fingerprint(&req.signed_bundle.bundle.account_identity_pubkey);

    let signed_bundle_json = serde_json::to_string(&req.signed_bundle)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("serialize signed bundle: {}", e)))?;

    let now = chrono::Utc::now();
    let record = KeyDirectoryRecord {
        recipient_id: user.recipient_id,
        bundle_version: req.signed_bundle.bundle.bundle_version,
        bundle_expiry: req.signed_bundle.bundle.bundle_expiry,
        aik_fingerprint_hex: aik_fingerprint_hex.clone(),
        signed_bundle_json,
        enc_sk_b64: req.enc_sk_b64,
        enc_sk_recovery_b64: req.enc_sk_recovery_b64,
        created_at: now,
    };

    let store = KeyDirectoryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    store
        .put(&record)
        .await
        .map_err(|e| ApiError::Conflict(format!("rollback or write failure: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(KeyBundleResponse {
            recipient_id: user.recipient_id,
            aik_fingerprint_hex,
            bundle_version: req.signed_bundle.bundle.bundle_version,
            bundle_expiry: req.signed_bundle.bundle.bundle_expiry,
            signed_bundle: req.signed_bundle,
        }),
    ))
}

/// AIK fingerprint stored server-side: lowercase hex of full SHA-256 of the AIK
/// uncompressed pubkey (256 bits / 64 hex chars). Used for AIK-continuity enforcement
/// on bundle rotation in `KeyDirectoryStore::put`.
///
/// Clients receive this hex value via `KeyBundleResponse.aik_fingerprint_hex` and
/// derive the human-verification display form (26 Crockford Base32 chars, ≥128-bit
/// second-preimage / ~64-bit collision resistance per architecture §"Fingerprint
/// Verification"). The truncation happens client-side because the full hex is needed
/// for AIK-continuity comparison server-side.
fn aik_fingerprint(aik_pubkey: &[u8]) -> String {
    let mut ctx = DigestContext::new(&SHA256);
    ctx.update(aik_pubkey);
    let digest = ctx.finish();
    // Explicit lowercase normalization — DynamoDB compares strings case-sensitively,
    // so storage and comparison must always use the same case (M-9).
    digest.as_ref().iter().map(|b| format!("{:02x}", b)).collect::<String>().to_ascii_lowercase()
}
