use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::db::sessions::SessionStore;
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthenticatedUser;
use crate::models::session::SessionView;
use crate::state::AppState;

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> ApiResult<Json<Vec<SessionView>>> {
    let store = SessionStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let sessions = store
        .list_for_recipient(user.recipient_id)
        .await
        .map_err(|e| ApiError::Internal(e))?;
    Ok(Json(sessions))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(token_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // Delete the specific session row AND decrement the counter, atomically.
    // Only the owning recipient can do this (the recipient_id key partition is implicit
    // via the authenticated user's recipient_id).
    let store = SessionStore::new(state.ddb(), &state.cfg().dynamodb_table_prefix);
    let deleted = store
        .delete_session_atomic(user.recipient_id, &token_id.to_string())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("delete session error: {}", e)))?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
