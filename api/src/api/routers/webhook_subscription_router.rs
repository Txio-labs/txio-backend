use crate::api::handlers::webhook_subscription_handler;
use crate::services::webhook_service::WebhookService;
use axum::{routing::{get, post}, Router};

pub fn router(service: WebhookService) -> Router {
    Router::new()
        .route("/", post(webhook_subscription_handler::create_webhook))
        .route("/", get(webhook_subscription_handler::list_webhooks))
        .route("/:id", axum::routing::delete(webhook_subscription_handler::delete_webhook))
        .with_state(service)
}
