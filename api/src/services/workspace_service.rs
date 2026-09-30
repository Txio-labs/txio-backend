use crate::dtos::workspace_dtos::{
    CommentResponse, InvitePreviewResponse, InviteResponse, MemberResponse, WorkspaceWithRole,
};
use crate::model::workspace::{Workspace, WorkspaceType};
use crate::model::workspace_comment::WorkspaceComment;
use crate::model::workspace_member::{WorkspaceMember, WorkspaceRole, STATUS_PENDING};
use crate::repositories::{
    collection_repository::CollectionRepository, history_repository::HistoryRepository,
    request_repository::RequestRepository, user_repository::UserRepository,
    workspace_member_repository::WorkspaceMemberRepository, workspace_repository::WorkspaceRepository,
};
use crate::services::email_service::EmailService;
use crate::services::workspace_access::{hash_invite_token, new_invite_token, WorkspaceAccess};
use crate::utils::error::AppError;
use chrono::{Duration, Utc};
use mongodb::bson::oid::ObjectId;

const INVITE_VALID_DAYS: i64 = 7;
/// Bounds how much a single workspace owner can make us email.
const MAX_PENDING_INVITES: usize = 20;

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Clone)]
pub struct WorkspaceService {
    workspace_repo: WorkspaceRepository,
    collection_repo: CollectionRepository,
    request_repo: RequestRepository,
    history_repo: HistoryRepository,
    member_repo: WorkspaceMemberRepository,
    user_repo: UserRepository,
    access: WorkspaceAccess,
    email_service: EmailService,
    frontend_url: String,
}

impl WorkspaceService {
    pub fn new(
        workspace_repo: WorkspaceRepository,
        collection_repo: CollectionRepository,
        request_repo: RequestRepository,
        history_repo: HistoryRepository,
        member_repo: WorkspaceMemberRepository,
        user_repo: UserRepository,
        email_service: EmailService,
        frontend_url: String,
    ) -> Self {
        let access = WorkspaceAccess::new(workspace_repo.clone(), member_repo.clone());
        Self {
            workspace_repo,
            collection_repo,
            request_repo,
            history_repo,
            member_repo,
            user_repo,
            access,
            email_service,
            frontend_url,
        }
    }

    pub fn access(&self) -> WorkspaceAccess {
        self.access.clone()
    }

    pub async fn create_workspace(
        &self,
        user_id: ObjectId,
        name: String,
        workspace_type: WorkspaceType,
    ) -> Result<Workspace, AppError> {
        let existing_workspaces = self.workspace_repo.find_all_by_user(user_id).await?;

        let workspace = self
            .workspace_repo
            .save(&Workspace::new(user_id, name, workspace_type))
            .await?;

        if existing_workspaces.is_empty() {
            if let Some(workspace_id) = workspace.id {
                self.collection_repo
                    .assign_workspace_to_unscoped_user_collections(user_id, workspace_id)
                    .await?;
            }
        }

        Ok(workspace)
    }

    /// Workspaces the user owns and workspaces they were invited to, each
    /// with the role they hold in it.
    pub async fn get_user_workspaces(&self, user_id: ObjectId) -> Result<Vec<WorkspaceWithRole>, AppError> {
        let mut out: Vec<WorkspaceWithRole> = self
            .workspace_repo
            .find_all_by_user(user_id)
            .await?
            .into_iter()
            .map(|workspace| WorkspaceWithRole { workspace, role: WorkspaceRole::Owner })
            .collect();

        for membership in self.member_repo.list_active_for_user(user_id).await? {
            // A workspace deleted since the invite was accepted is skipped, not an error.
            if let Ok(workspace) = self.workspace_repo.find_by_id(membership.workspace_id).await {
                out.push(WorkspaceWithRole { workspace, role: membership.role });
            }
        }
        out.sort_by_key(|w| w.workspace.created_at);
        Ok(out)
    }

    pub async fn get_workspace_for_user(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<Workspace, AppError> {
        let workspace = self.workspace_repo.find_by_id(workspace_id).await?;

        if workspace.user_id != user_id {
            return Err(AppError::Forbidden(
                "Not authorized to access this workspace".into(),
            ));
        }

        Ok(workspace)
    }

    pub async fn rename_workspace(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
        name: String,
    ) -> Result<Workspace, AppError> {
        let mut workspace = self.get_workspace_for_user(workspace_id, user_id).await?;
        workspace.name = name;
        workspace.updated_at = chrono::Utc::now();
        self.workspace_repo.update(&workspace).await
    }

    /// Deletes a workspace and everything scoped to it (collections, their
    /// saved requests, and history entries) — mirrors `CollectionService::
    /// delete_collection`'s cascade, just one level up. Refuses to delete a
    /// user's last remaining workspace so every account always has at least
    /// one to operate in.
    pub async fn delete_workspace(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(), AppError> {
        self.get_workspace_for_user(workspace_id, user_id).await?;

        let workspace_count = self.workspace_repo.count_by_user(user_id).await?;
        if workspace_count <= 1 {
            return Err(AppError::BadRequest(
                "Cannot delete your only workspace".into(),
            ));
        }

        let collections = self
            .collection_repo
            .find_all_by_workspace(workspace_id)
            .await?;
        for collection in &collections {
            if let Some(collection_id) = collection.id {
                self.request_repo
                    .delete_all_by_collection(collection_id)
                    .await?;
            }
        }
        self.collection_repo
            .delete_all_by_workspace(workspace_id)
            .await?;

        self.history_repo
            .delete_all_by_user(user_id, Some(workspace_id))
            .await?;

        self.member_repo.delete_all_for_workspace(workspace_id).await?;
        self.workspace_repo.delete(workspace_id).await?;

        Ok(())
    }

    // --- Members and invitations ---

    fn member_response(member: &WorkspaceMember) -> MemberResponse {
        MemberResponse {
            id: member.id.map(|i| i.to_hex()),
            email: member.email.clone(),
            role: member.role,
            status: member.status.clone(),
            expires_at: member.expires_at.map(|t| t.to_rfc3339()),
        }
    }

    /// Everyone in the workspace, owner first. Any member may see the list.
    pub async fn list_members(&self, workspace_id: ObjectId, user_id: ObjectId) -> Result<Vec<MemberResponse>, AppError> {
        let (workspace, role) = self.access.require(workspace_id, user_id, WorkspaceRole::Viewer).await?;
        let owner = self.user_repo.find_by_id(&workspace.user_id).await?;

        let mut out = vec![MemberResponse {
            id: None,
            email: owner.email,
            role: WorkspaceRole::Owner,
            status: "owner".into(),
            expires_at: None,
        }];
        for member in self.member_repo.list_for_workspace(workspace_id).await? {
            // Pending invites (and their addresses) are for the owner to see.
            if member.status == STATUS_PENDING && role != WorkspaceRole::Owner {
                continue;
            }
            out.push(Self::member_response(&member));
        }
        Ok(out)
    }

    /// Invites `email` to the workspace, or re-sends and re-issues an open invite.
    pub async fn invite_member(
        &self,
        workspace_id: ObjectId,
        inviter_id: ObjectId,
        inviter_email: &str,
        email: &str,
        role: &str,
    ) -> Result<InviteResponse, AppError> {
        let (workspace, _) = self.access.require(workspace_id, inviter_id, WorkspaceRole::Owner).await?;
        let role = WorkspaceRole::parse_invitable(role)
            .ok_or_else(|| AppError::BadRequest("Role must be \"viewer\" or \"editor\"".into()))?;
        let email = normalize_email(email);

        let owner = self.user_repo.find_by_id(&workspace.user_id).await?;
        if email == normalize_email(&owner.email) {
            return Err(AppError::BadRequest("You already own this workspace".into()));
        }

        let existing = self.member_repo.list_for_workspace(workspace_id).await?;
        let pending = existing.iter().filter(|m| m.status == STATUS_PENDING).count();
        let reissuing = existing.iter().any(|m| m.email == email && m.status == STATUS_PENDING);
        if !reissuing && pending >= MAX_PENDING_INVITES {
            return Err(AppError::BadRequest("Too many open invitations. Revoke some first.".into()));
        }

        let (token, token_hash) = new_invite_token();
        let member = self
            .member_repo
            .upsert_pending(workspace_id, &email, role, inviter_id, &token_hash, Utc::now() + Duration::days(INVITE_VALID_DAYS))
            .await?;

        let accept_link = format!("{}/workspace#invite={token}", self.frontend_url.trim_end_matches('/'));
        let role_label = if role == WorkspaceRole::Editor { "an editor" } else { "a viewer" };
        let email_sent = match self
            .email_service
            .send_workspace_invite_email(&email, &workspace.name, inviter_email, role_label, &accept_link)
            .await
        {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "Workspace invite email failed");
                false
            }
        };

        Ok(InviteResponse { member: Self::member_response(&member), email_sent, accept_link })
    }

    /// Shows what an invite is for, before the invitee signs in. The token is
    /// the credential; nothing is revealed without it.
    pub async fn preview_invite(&self, token: &str) -> Result<InvitePreviewResponse, AppError> {
        let invite = self.member_repo.find_pending_by_token_hash(&hash_invite_token(token)).await?
            .ok_or_else(|| AppError::NotFound("This invitation is invalid or has expired".into()))?;
        let workspace = self.workspace_repo.find_by_id(invite.workspace_id).await
            .map_err(|_| AppError::NotFound("This invitation is invalid or has expired".into()))?;
        Ok(InvitePreviewResponse { workspace_name: workspace.name, invited_email: invite.email, role: invite.role })
    }

    /// Accepts an invite as the signed-in user. It only works for the account
    /// whose email the invite was sent to, and only once.
    pub async fn accept_invite(&self, token: &str, user_id: ObjectId, user_email: &str) -> Result<Workspace, AppError> {
        let invite = self.member_repo.find_pending_by_token_hash(&hash_invite_token(token)).await?
            .ok_or_else(|| AppError::NotFound("This invitation is invalid or has expired".into()))?;

        if invite.email != normalize_email(user_email) {
            return Err(AppError::Forbidden("This invitation was sent to a different email address".into()));
        }
        let workspace = self.workspace_repo.find_by_id(invite.workspace_id).await
            .map_err(|_| AppError::NotFound("This invitation is invalid or has expired".into()))?;
        if workspace.user_id == user_id {
            return Err(AppError::BadRequest("You already own this workspace".into()));
        }

        let member_id = invite.id.ok_or_else(|| AppError::InternalError("Invite without id".into()))?;
        self.member_repo.activate(member_id, user_id).await?;
        Ok(workspace)
    }

    /// Removes a member or cancels a pending invite. Owner only. Access ends
    /// with the next request: every check reads the membership row.
    pub async fn remove_member(&self, workspace_id: ObjectId, owner_id: ObjectId, member_id: ObjectId) -> Result<(), AppError> {
        self.access.require(workspace_id, owner_id, WorkspaceRole::Owner).await?;
        let member = self.member_repo.find_in_workspace(workspace_id, member_id).await?;
        self.member_repo.delete(member.id.unwrap_or(member_id)).await
    }

    pub async fn set_member_role(&self, workspace_id: ObjectId, owner_id: ObjectId, member_id: ObjectId, role: &str) -> Result<(), AppError> {
        self.access.require(workspace_id, owner_id, WorkspaceRole::Owner).await?;
        let role = WorkspaceRole::parse_invitable(role)
            .ok_or_else(|| AppError::BadRequest("Role must be \"viewer\" or \"editor\"".into()))?;
        let member = self.member_repo.find_in_workspace(workspace_id, member_id).await?;
        self.member_repo.set_role(member.id.unwrap_or(member_id), role).await
    }

    /// A member leaves on their own. The owner cannot: they would have to delete the workspace.
    pub async fn leave(&self, workspace_id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let workspace = self.workspace_repo.find_by_id(workspace_id).await
            .map_err(|_| AppError::Forbidden("Not authorized to access this workspace".into()))?;
        if workspace.user_id == user_id {
            return Err(AppError::BadRequest("The owner cannot leave. Delete the workspace instead.".into()));
        }
        let membership = self.member_repo.find_active(workspace_id, user_id).await?
            .ok_or_else(|| AppError::Forbidden("Not authorized to access this workspace".into()))?;
        self.member_repo.delete(membership.id.ok_or_else(|| AppError::InternalError("Member without id".into()))?).await
    }

    // --- Comments (visible to members of the workspace only) ---

    pub async fn list_comments(&self, workspace_id: ObjectId, user_id: ObjectId, target: &str) -> Result<Vec<CommentResponse>, AppError> {
        let (_, role) = self.access.require(workspace_id, user_id, WorkspaceRole::Viewer).await?;
        Ok(self
            .member_repo
            .list_comments(workspace_id, target)
            .await?
            .into_iter()
            .map(|c| Self::comment_response(c, user_id, role))
            .collect())
    }

    pub async fn add_comment(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
        user_email: &str,
        target_id: String,
        body: String,
    ) -> Result<CommentResponse, AppError> {
        // Viewers may comment: it is how they take part without edit rights.
        let (_, role) = self.access.require(workspace_id, user_id, WorkspaceRole::Viewer).await?;
        let comment = WorkspaceComment {
            id: None,
            workspace_id,
            target_id,
            author_id: user_id,
            author_email: normalize_email(user_email),
            body: body.trim().to_string(),
            created_at: Utc::now(),
        };
        let saved = self.member_repo.add_comment(&comment).await?;
        Ok(Self::comment_response(saved, user_id, role))
    }

    pub async fn delete_comment(&self, workspace_id: ObjectId, user_id: ObjectId, comment_id: ObjectId) -> Result<(), AppError> {
        let (_, role) = self.access.require(workspace_id, user_id, WorkspaceRole::Viewer).await?;
        let comment = self.member_repo.find_comment(workspace_id, comment_id).await?;
        if comment.author_id != user_id && role != WorkspaceRole::Owner {
            return Err(AppError::Forbidden("Only the author or the workspace owner can delete a comment".into()));
        }
        self.member_repo.delete_comment(comment_id).await
    }

    fn comment_response(c: WorkspaceComment, viewer: ObjectId, role: WorkspaceRole) -> CommentResponse {
        CommentResponse {
            id: c.id.map(|i| i.to_hex()).unwrap_or_default(),
            target_id: c.target_id,
            author_email: c.author_email,
            body: c.body,
            created_at: c.created_at.to_rfc3339(),
            can_delete: c.author_id == viewer || role == WorkspaceRole::Owner,
        }
    }
}
