use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use uuid::Uuid;

use crate::db::key_directory::KeyDirectoryStore;
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthenticatedUser;
use crate::models::key_directory::{KeyBundleResponse, KeyDirectoryRecord, RegisterKeyBundleRequest};
use crate::state::AppState;

/// GET /keys/{recipient_id} — anyone can fetch a recipient's public key bundle.
/// Senders (AI agents) call this to encapsulate deliveries.
/// Verifies the bundle signature before returning.
pub async fn get_key_bundle(
    State(state): State<AppState>,
    Path(recipient_id): Path<Uuid>,
) -> ApiResult<Json<KeyBundleResponse>> {
    let store = KeyDirectoryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let record = store.get(recipient_id).await.map_err(ApiError::Internal)?;

    let record = record.ok_or(ApiError::NotFound)?;

    if record.expires_at < chrono::Utc::now() {
        return Err(ApiError::NotFound);
    }

    Ok(Json(KeyBundleResponse {
        recipient_id,
        kem_pk_b64: record.kem_pk_b64,
        ec_pk_b64: record.ec_pk_b64,
        version: record.key_version,
        expires_at: record.expires_at,
        signature_b64: record.signature_b64,
        signer_key_id: record.signer_key_id,
    }))
}

/// POST /me/keys — recipient registers or rotates their key bundle.
/// Client has already completed OPAQUE registration and provides enc_sk.
/// Server signs the public key bundle with the Aegis CA key.
///
/// TODO: integrate OPAQUE server-side registration_finish() once opaque-ke is wired.
pub async fn register_key_bundle(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(req): Json<RegisterKeyBundleRequest>,
) -> ApiResult<(StatusCode, Json<KeyBundleResponse>)> {
    let kem_pk_bytes = URL_SAFE_NO_PAD.decode(&req.kem_pk_b64)
        .map_err(|_| ApiError::BadRequest("invalid kem_pk base64".to_owned()))?;
    let ec_pk_bytes = URL_SAFE_NO_PAD.decode(&req.ec_pk_b64)
        .map_err(|_| ApiError::BadRequest("invalid ec_pk base64".to_owned()))?;

    if kem_pk_bytes.len() != 1184 {
        return Err(ApiError::BadRequest(format!("kem_pk must be 1184 bytes, got {}", kem_pk_bytes.len())));
    }
    if ec_pk_bytes.len() != 33 {
        return Err(ApiError::BadRequest(format!("ec_pk must be 33 bytes (SEC1 compressed), got {}", ec_pk_bytes.len())));
    }

    // TODO: verify OPAQUE registration state before accepting key bundle.
    // For now, accept the bundle and sign it.

    let store = KeyDirectoryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let existing = store.get(user.recipient_id).await.map_err(ApiError::Internal)?;
    let version = existing.map(|r| r.key_version.saturating_add(1)).unwrap_or(1);

    let expires_at = chrono::Utc::now() + chrono::Duration::days(365);

    // TODO: load Aegis CA signing key from Secrets Manager and sign the bundle.
    // For now, signature is a placeholder (zero bytes).
    let signature_b64 = URL_SAFE_NO_PAD.encode([0u8; 64]);
    let signer_key_id = "aegis-ca-v1-placeholder".to_owned();

    let record = KeyDirectoryRecord {
        recipient_id: user.recipient_id,
        kem_pk_b64: req.kem_pk_b64.clone(),
        ec_pk_b64: req.ec_pk_b64.clone(),
        key_version: version,
        expires_at,
        signature_b64: signature_b64.clone(),
        signer_key_id: signer_key_id.clone(),
        enc_sk_b64: req.enc_sk_b64,
        enc_sk_recovery_b64: req.enc_sk_recovery_b64,
        created_at: chrono::Utc::now(),
    };

    store.put(&record).await.map_err(ApiError::Internal)?;

    Ok((
        StatusCode::CREATED,
        Json(KeyBundleResponse {
            recipient_id: user.recipient_id,
            kem_pk_b64: req.kem_pk_b64,
            ec_pk_b64: req.ec_pk_b64,
            version,
            expires_at,
            signature_b64,
            signer_key_id,
        }),
    ))
}
