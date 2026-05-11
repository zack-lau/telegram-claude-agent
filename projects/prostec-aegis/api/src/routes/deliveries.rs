use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::db::deliveries::DeliveryStore;
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthenticatedUser;
use crate::models::delivery::{CreateDeliveryRequest, CreateDeliveryResponse, DeliveryResponse};
use crate::state::AppState;

/// POST /deliveries — AI agent records that an encrypted delivery happened.
///
/// The agent uploads the encrypted envelope to the recipient's cloud storage
/// directly. This endpoint only records that a delivery occurred (metadata only).
/// Aegis never receives or stores the encrypted body.
///
/// TODO: API key auth middleware for agent callers.
pub async fn create_delivery(
    State(_state): State<AppState>,
    Json(_req): Json<CreateDeliveryRequest>,
) -> ApiResult<(StatusCode, Json<CreateDeliveryResponse>)> {
    Err(ApiError::BadRequest(
        "delivery creation not yet implemented — pending key bundle fetch + envelope seal integration".to_owned(),
    ))
}

/// GET /deliveries/{delivery_id} — recipient fetches delivery metadata.
///
/// Returns metadata: delivery_id, doc_id (use this for dedup — not delivery_id),
/// sender_id, recipient_id, provider, provider_file_id, size_bytes, delivered_at.
/// No body content is returned — the encrypted envelope lives in the recipient's
/// cloud storage at provider_file_id.
pub async fn get_delivery(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(delivery_id): Path<Uuid>,
) -> ApiResult<Json<DeliveryResponse>> {
    let store = DeliveryStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let record = store
        .get(delivery_id, user.recipient_id)
        .await
        .map_err(|e| {
            // Surface legacy-format rows as BadRequest, not 500.
            let msg = e.to_string();
            if msg.starts_with("legacy_cloud_path:") {
                ApiError::BadRequest(msg)
            } else {
                ApiError::Internal(e)
            }
        })?
        .ok_or(ApiError::NotFound)?;

    if record.expires_at < chrono::Utc::now() {
        return Err(ApiError::NotFound);
    }

    Ok(Json(DeliveryResponse {
        delivery_id,
        doc_id: record.doc_id,
        sender_id: record.sender_id,
        recipient_id: user.recipient_id,
        provider: record.provider,
        provider_file_id: record.provider_file_id,
        size_bytes: record.size_bytes,
        delivered_at: record.delivered_at,
    }))
}
