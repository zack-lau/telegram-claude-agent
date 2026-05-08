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

/// Injected into request extensions by the auth middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// Cognito sub (stable UUID per user per pool) = recipient_id.
    pub recipient_id: Uuid,
    pub raw_sub: String,
    pub groups: Vec<String>,
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
    let user = AuthenticatedUser {
        recipient_id,
        raw_sub: claims.sub,
        groups: claims.groups,
    };

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
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

/// For API key clients (AI agents). Validates against the `api_keys` DynamoDB table.
/// Stub — full implementation in routes/api_keys.rs.
pub async fn require_api_key(
    State(_state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let _key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;

    // TODO: look up key in DDB, check active, inject AgentIdentity extension
    Err(ApiError::BadRequest("api key auth not yet implemented".to_owned()))
}
