use crate::api::handlers::stellar_handler;
use crate::services::stellar_indexer::StellarIndexer;
use axum::{Router, routing::get};

pub fn router(indexer: StellarIndexer) -> Router {
    Router::new()
        .route("/contracts/:id/events", get(stellar_handler::get_contract_events))
        .route("/accounts/:address/tokens", get(stellar_handler::get_token_balances))
        .with_state(indexer)
}
