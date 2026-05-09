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

use crate::middleware::auth::require_auth;
use crate::middleware::request_id::inject_request_id;
use crate::state::AppState;

mod deliveries;
mod envelopes;
mod health;
mod keys;
mod sessions;

pub fn router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/deliveries/{delivery_id}", get(deliveries::get_delivery))
        .route("/envelopes/fetch", post(envelopes::fetch_envelope))
        .route("/me/sessions", get(sessions::list_sessions))
        .route("/me/sessions/{token_id}", delete(sessions::revoke_session))
        .route("/me/keys", post(keys::register_key_bundle))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/health", get(health::health))
        .route("/keys/{recipient_id}", get(keys::get_key_bundle))
        // agent routes (API key auth — stub for now)
        .route("/deliveries", post(deliveries::create_delivery))
        .merge(authed)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(CorsLayer::permissive()) // TODO: tighten for production
        .layer(middleware::from_fn(inject_request_id))
        .with_state(state)
}
