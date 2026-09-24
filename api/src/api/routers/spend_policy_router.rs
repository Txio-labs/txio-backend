use crate::api::handlers::spend_policy_handler;
use crate::services::spend_policy_service::SpendPolicyService;
use axum::{
    routing::{get, post},
    Router,
};

pub fn router(service: SpendPolicyService) -> Router {
    Router::new()
        .route("/", post(spend_policy_handler::upsert_policy))
        .route("/", get(spend_policy_handler::get_policy))
        .route("/", axum::routing::delete(spend_policy_handler::delete_policy))
        .route("/check", post(spend_policy_handler::check_spend))
        .with_state(service)
}
