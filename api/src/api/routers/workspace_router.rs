use crate::api::handlers::workspace_handler;
use crate::services::workspace_service::WorkspaceService;
use axum::{
    routing::{delete, get, patch, post, put},
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
        // Invitations. `preview` needs no session (the invitee may be new);
        // `accept` needs one, and only works for the invited email.
        .route("/invites/preview", get(workspace_handler::preview_invite))
        .route("/invites/accept", post(workspace_handler::accept_invite))
        // Members
        .route("/:id/members", get(workspace_handler::list_members))
        .route("/:id/members/invite", post(workspace_handler::invite_member))
        .route(
            "/:id/members/:member_id",
            patch(workspace_handler::update_member_role).delete(workspace_handler::remove_member),
        )
        .route("/:id/leave", post(workspace_handler::leave_workspace))
        // Comments
        .route(
            "/:id/comments",
            get(workspace_handler::list_comments).post(workspace_handler::add_comment),
        )
        .route("/:id/comments/:comment_id", delete(workspace_handler::delete_comment))
        .with_state(service)
}
