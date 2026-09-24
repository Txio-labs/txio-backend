use crate::api::handlers::session_key_handler;
use crate::services::session_key_service::SessionKeyService;
use axum::{
    routing::{get, post},
    Router,
};

pub fn router(service: SessionKeyService) -> Router {
    Router::new()
        .route("/", post(session_key_handler::create_session_key))
        .route("/", get(session_key_handler::list_session_keys))
        .route("/:id/revoke", post(session_key_handler::revoke_session_key))
        .with_state(service)
}
