use crate::api::handlers::api_key_handler;
use crate::services::api_key_service::ApiKeyService;
use axum::{routing::{get, post}, Router};

pub fn router(service: ApiKeyService) -> Router {
    Router::new()
        .route("/", post(api_key_handler::create_api_key))
        .route("/", get(api_key_handler::list_api_keys))
        .route("/:id", axum::routing::delete(api_key_handler::revoke_api_key))
        .with_state(service)
}
