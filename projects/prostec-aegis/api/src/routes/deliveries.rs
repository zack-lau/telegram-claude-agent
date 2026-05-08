use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use uuid::Uuid;

use crate::db::deliveries::DeliveryStore;
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthenticatedUser;
use crate::models::delivery::{CreateDeliveryRequest, CreateDeliveryResponse, DeliveryResponse};
use crate::state::AppState;

/// POST /deliveries — AI agent creates an encrypted delivery.
///
/// The agent provides a plaintext payload and a list of recipient IDs.
/// This endpoint:
///   1. Fetches each recipient's signed key bundle from the key directory
///   2. Verifies bundle signatures against the Aegis CA
///   3. Seals the envelope (KEM + AEAD per ADR 0002)
///   4. Stores the envelope header in DynamoDB
///   5. Uploads the encrypted body to S3
///
/// TODO: API key auth middleware for agent callers.
/// TODO: full sealing pipeline (key bundle fetch + envelope::seal).
pub async fn create_delivery(
    State(_state): State<AppState>,
    Json(_req): Json<CreateDeliveryRequest>,
) -> ApiResult<(StatusCode, Json<CreateDeliveryResponse>)> {
    Err(ApiError::BadRequest(
        "delivery creation not yet implemented — pending key bundle fetch + envelope seal integration".to_owned(),
    ))
}

/// GET /deliveries/{delivery_id} — recipient fetches an encrypted delivery.
///
/// Returns the envelope header (contains recipient-specific KEM slot) and
/// the encrypted body ciphertext from S3. Decryption happens client-side.
///
/// On first fetch:
///   - Records decrypted_at + token_id in DynamoDB (audit log)
///   - If burn_after_read: deletes the S3 body object immediately
///   - Emits audit log entry
///
/// TODO: burn-after-read S3 delete, audit log emit, re-fetch email notification.
pub async fn get_delivery(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(delivery_id): Path<Uuid>,
) -> ApiResult<Json<DeliveryResponse>> {
    let store = DeliveryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let record = store
        .get(delivery_id, user.recipient_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;

    if record.expires_at < chrono::Utc::now() {
        return Err(ApiError::NotFound);
    }

    // Fetch encrypted body from S3
    let body_obj = state.s3()
        .get_object()
        .bucket(&state.cfg().s3_delivery_bucket)
        .key(record.content_id.to_string())
        .send()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("s3 get error: {}", e)))?;

    let body_bytes = body_obj
        .body
        .collect()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("s3 body read error: {}", e)))?
        .into_bytes();

    // Body wire format: [12B nonce][ciphertext]
    if body_bytes.len() < 12 {
        return Err(ApiError::Internal(anyhow::anyhow!("body object too short")));
    }
    let (nonce, ciphertext) = body_bytes.split_at(12);

    Ok(Json(DeliveryResponse {
        delivery_id,
        sender_id: record.sender_id,
        created_at: record.created_at,
        expires_at: record.expires_at,
        burn_after_read: record.burn_after_read,
        envelope_header: record.envelope_header,
        body_ciphertext_b64: STANDARD.encode(ciphertext),
        body_nonce_b64: STANDARD.encode(nonce),
    }))
}
