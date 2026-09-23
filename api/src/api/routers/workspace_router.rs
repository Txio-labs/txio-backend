use crate::api::handlers::workspace_handler;
use crate::services::workspace_service::WorkspaceService;
use axum::{
    routing::{get, post, put},
    Router,
};

pub fn router(service: WorkspaceService) -> Router {
    Router::new()
        .route("/", get(workspace_handler::get_user_workspaces))
        .route("/", post(workspace_handler::create_workspace))
        .route(
            "/:id",
            put(workspace_handler::update_workspace).delete(workspace_handler::delete_workspace),
        )
        .with_state(service)
}
