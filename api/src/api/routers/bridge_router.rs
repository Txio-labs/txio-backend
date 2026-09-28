use axum::{
    routing::{get, post},
    Router,
};

use crate::api::handlers::bridge_handler;
use crate::services::{bridge_service::BridgeService, offramp_service::OfframpService};

pub fn router(bridge_service: BridgeService, offramp_service: OfframpService) -> Router {
    let swap_routes = Router::new()
        .route("/quote", post(bridge_handler::get_quote))
        .route("/orders", post(bridge_handler::execute_order))
        // axum 0.7 (this workspace's pinned version) uses `:param`, not the
        // `{param}` syntax introduced in 0.8 — `{param}` here would match
        // only the literal string "/orders/{provider_order_id}".
        .route(
            "/orders/:provider_order_id",
            get(bridge_handler::get_order_status),
        )
        .with_state(bridge_service);

    let offramp_routes = Router::new()
        .route("/quote", post(bridge_handler::get_offramp_quote))
        .with_state(offramp_service);

    Router::new()
        .nest("/swap", swap_routes)
        .nest("/offramp", offramp_routes)
}
