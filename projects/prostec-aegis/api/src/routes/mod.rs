use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use std::time::Duration;

use crate::middleware::auth::{require_api_key, require_auth};
use crate::middleware::dpop::require_dpop_session_bound;
use crate::middleware::request_id::inject_request_id;
use crate::state::AppState;

mod auth;
mod deliveries;
mod envelopes;
mod health;
mod keys;
mod sessions;
mod streaming;

pub fn router(state: AppState) -> Router {
    // Fix H10 — explicit CORS origins from config; default is empty (no CORS).
    let cors = build_cors_layer(state.cfg().cors_allowed_origins.as_slice());

    let authed = Router::new()
        .route("/auth/sessions", post(auth::create_session))
        .route("/auth/sessions/refresh", post(auth::refresh_session))
        .route("/auth/logout", post(auth::logout))
        .route("/deliveries", post(deliveries::create_delivery))
        .route("/deliveries/{delivery_id}", get(deliveries::get_delivery))
        .route("/envelopes/fetch", post(envelopes::fetch_envelope))
        .route("/me/sessions", get(sessions::list_sessions))
        .route("/me/sessions/{token_id}", delete(sessions::revoke_session))
        .route("/me/keys", post(keys::register_key_bundle))
        // Stack: require_auth (Cognito JWT + epoch + fips_strict) → DPoP session binding
        // (opt-in via X-Aegis-Session-Id header). Layer order: bottom-up. require_dpop
        // runs AFTER require_auth so it can read AuthenticatedUser.
        .route_layer(middleware::from_fn_with_state(state.clone(), require_dpop_session_bound))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    // Agent-facing endpoints — API key auth (NOT Cognito). Streaming upload sessions for
    // chunked envelope encryption (architecture §"Streaming Encryption").
    let agent_authed = Router::new()
        .route("/v1/streaming/init", post(streaming::init))
        .route("/v1/streaming/{upload_uuid}/complete", post(streaming::complete))
        .route("/v1/streaming/{upload_uuid}", delete(streaming::abort))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_api_key));

    Router::new()
        .route("/health", get(health::health))
        .route("/keys/{recipient_id}", get(keys::get_key_bundle))
        .merge(authed)
        .merge(agent_authed)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(cors)
        .layer(middleware::from_fn(inject_request_id))
        .with_state(state)
}

fn build_cors_layer(allowed_origins: &[String]) -> CorsLayer {
    use tower_http::cors::AllowOrigin;

    if allowed_origins.is_empty() {
        // No origins configured — explicitly deny all cross-origin requests.
        // CorsLayer::new() defaults to no allow_origin in tower-http 0.5+ which
        // already blocks at the browser, but make it explicit so a future
        // tower-http version change can't widen the default (Qwen R3+R4 routes).
        return CorsLayer::new().allow_origin(AllowOrigin::list(Vec::<axum::http::HeaderValue>::new()));
    }

    let origins: Vec<axum::http::HeaderValue> = allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    CorsLayer::new().allow_origin(AllowOrigin::list(origins))
}
