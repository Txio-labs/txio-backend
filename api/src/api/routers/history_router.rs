use crate::api::handlers::history_handler;
use crate::services::history_service::HistoryService;
use axum::{
    Router,
    routing::{delete, get, post},
};

pub fn router(service: HistoryService) -> Router {
    Router::new()
        .route("/", post(history_handler::create_history_entry))
        .route("/", get(history_handler::get_history))
        .route("/", delete(history_handler::clear_history))
        .route("/:id", delete(history_handler::delete_history_entry))
        .with_state(service)
}
