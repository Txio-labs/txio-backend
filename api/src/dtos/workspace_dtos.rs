use crate::model::workspace::{Workspace, WorkspaceType};
use crate::model::workspace_member::WorkspaceRole;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateWorkspaceRequest {
    #[validate(length(
        min = 2,
        max = 48,
        message = "Workspace name must be between 2 and 48 characters"
    ))]
    pub name: String,
    pub workspace_type: Option<WorkspaceType>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceQuery {
    pub workspace_id: Option<String>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateWorkspaceRequest {
    #[validate(length(
        min = 2,
        max = 48,
        message = "Workspace name must be between 2 and 48 characters"
    ))]
    pub name: String,
}

/// A workspace as its viewer sees it: the workspace plus their role in it.
#[derive(Debug, Serialize)]
pub struct WorkspaceWithRole {
    #[serde(flatten)]
    pub workspace: Workspace,
    pub role: WorkspaceRole,
}

#[derive(Debug, Deserialize, Validate)]
pub struct InviteMemberRequest {
    #[validate(email(message = "Enter a valid email address"))]
    pub email: String,
    /// `viewer` or `editor`.
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemberRoleRequest {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct InvitePreviewQuery {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    /// Absent for the owner, who is not a membership row.
    pub id: Option<String>,
    pub email: String,
    pub role: WorkspaceRole,
    /// `owner`, `active` or `pending`.
    pub status: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InviteResponse {
    pub member: MemberResponse,
    /// False when the email provider rejected the message. The owner can
    /// still share `accept_link` themselves.
    pub email_sent: bool,
    pub accept_link: String,
}

#[derive(Debug, Serialize)]
pub struct InvitePreviewResponse {
    pub workspace_name: String,
    pub invited_email: String,
    pub role: WorkspaceRole,
}

#[derive(Debug, Deserialize)]
pub struct CommentsQuery {
    pub target: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct CreateCommentRequest {
    #[validate(length(min = 1, max = 400, message = "Comment must be 1 to 400 characters"))]
    pub target_id: String,
    #[validate(length(min = 1, max = 2000, message = "Comment must be 1 to 2000 characters"))]
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct CommentResponse {
    pub id: String,
    pub target_id: String,
    pub author_email: String,
    pub body: String,
    pub created_at: String,
    /// True when the caller may delete it (author or workspace owner).
    pub can_delete: bool,
}
