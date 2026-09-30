use crate::api::handlers::webhook_subscription_handler;
use crate::services::webhook_service::WebhookService;
use axum::{routing::{get, post}, Router};

pub fn router(service: WebhookService) -> Router {
    Router::new()
        .route("/", post(webhook_subscription_handler::create_webhook))
        .route("/", get(webhook_subscription_handler::list_webhooks))
        .route("/:id", axum::routing::delete(webhook_subscription_handler::delete_webhook))
        .route("/:id/rotate-secret", post(webhook_subscription_handler::rotate_secret))
        .route("/:id/deliveries", get(webhook_subscription_handler::list_deliveries))
        .route(
            "/:id/deliveries/:delivery_id/redeliver",
            post(webhook_subscription_handler::redeliver),
        )
        .with_state(service)
}
