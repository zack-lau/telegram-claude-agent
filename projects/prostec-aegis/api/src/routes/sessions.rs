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
    // Delete the specific session row. Only the owning recipient can do this.
    state.ddb()
        .delete_item()
        .table_name(state.cfg().table("oauth-tokens"))
        .key("recipient_id", aws_sdk_dynamodb::types::AttributeValue::S(user.recipient_id.to_string()))
        .key("token_id", aws_sdk_dynamodb::types::AttributeValue::S(token_id.to_string()))
        .condition_expression("recipient_id = :rid")
        .expression_attribute_values(":rid", aws_sdk_dynamodb::types::AttributeValue::S(user.recipient_id.to_string()))
        .send()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("delete session error: {}", e)))?;

    Ok(StatusCode::NO_CONTENT)
}
