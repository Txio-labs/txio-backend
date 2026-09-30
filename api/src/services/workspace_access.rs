//! Who may do what in a workspace. One place decides, so collections,
//! requests, comments and member management cannot each invent their own rule.

use crate::model::workspace::Workspace;
use crate::model::workspace_member::{WorkspaceMember, WorkspaceRole};
use crate::repositories::workspace_member_repository::WorkspaceMemberRepository;
use crate::repositories::workspace_repository::WorkspaceRepository;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;
use sha2::{Digest, Sha256};

/// The role `user_id` holds in a workspace: owner if they created it, else the
/// role of their active membership, else none.
pub fn effective_role(workspace_owner: ObjectId, user_id: ObjectId, membership: Option<&WorkspaceMember>) -> Option<WorkspaceRole> {
    if workspace_owner == user_id {
        return Some(WorkspaceRole::Owner);
    }
    membership
        .filter(|m| m.user_id == Some(user_id) && m.status == crate::model::workspace_member::STATUS_ACTIVE)
        .map(|m| m.role)
}

/// `Ok(role)` when the caller holds at least `min`; `Forbidden` otherwise.
/// The same error for "not a member" and "not enough role" so the response
/// does not reveal whether a workspace exists.
pub fn authorize(role: Option<WorkspaceRole>, min: WorkspaceRole) -> Result<WorkspaceRole, AppError> {
    match role {
        Some(r) if r >= min => Ok(r),
        _ => Err(AppError::Forbidden("Not authorized to access this workspace".into())),
    }
}

/// 256 random bits, hex. The raw token goes in the email; only its hash is stored.
pub fn new_invite_token() -> (String, String) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let hash = hash_invite_token(&token);
    (token, hash)
}

pub fn hash_invite_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Clone)]
pub struct WorkspaceAccess {
    workspaces: WorkspaceRepository,
    members: WorkspaceMemberRepository,
}

impl WorkspaceAccess {
    pub fn new(workspaces: WorkspaceRepository, members: WorkspaceMemberRepository) -> Self {
        Self { workspaces, members }
    }

    pub async fn role(&self, workspace: &Workspace, user_id: ObjectId) -> Result<Option<WorkspaceRole>, AppError> {
        if workspace.user_id == user_id {
            return Ok(Some(WorkspaceRole::Owner));
        }
        let Some(workspace_id) = workspace.id else { return Ok(None) };
        let membership = self.members.find_active(workspace_id, user_id).await?;
        Ok(effective_role(workspace.user_id, user_id, membership.as_ref()))
    }

    /// Loads the workspace and checks the caller holds at least `min`.
    pub async fn require(&self, workspace_id: ObjectId, user_id: ObjectId, min: WorkspaceRole) -> Result<(Workspace, WorkspaceRole), AppError> {
        let workspace = match self.workspaces.find_by_id(workspace_id).await {
            Ok(w) => w,
            // Same answer as a workspace the caller cannot see.
            Err(AppError::NotFound(_)) => return Err(AppError::Forbidden("Not authorized to access this workspace".into())),
            Err(e) => return Err(e),
        };
        let role = authorize(self.role(&workspace, user_id).await?, min)?;
        Ok((workspace, role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::workspace_member::{STATUS_ACTIVE, STATUS_PENDING};
    use chrono::Utc;

    fn member(user: Option<ObjectId>, role: WorkspaceRole, status: &str) -> WorkspaceMember {
        WorkspaceMember {
            id: None,
            workspace_id: ObjectId::new(),
            email: "a@x.com".into(),
            user_id: user,
            role,
            status: status.into(),
            invited_by: ObjectId::new(),
            token_hash: None,
            expires_at: None,
            created_at: Utc::now(),
            accepted_at: None,
        }
    }

    #[test]
    fn the_workspace_creator_is_always_the_owner() {
        let owner = ObjectId::new();
        assert_eq!(effective_role(owner, owner, None), Some(WorkspaceRole::Owner));
    }

    #[test]
    fn active_members_get_their_role_and_strangers_get_none() {
        let (owner, user, other) = (ObjectId::new(), ObjectId::new(), ObjectId::new());
        let m = member(Some(user), WorkspaceRole::Editor, STATUS_ACTIVE);
        assert_eq!(effective_role(owner, user, Some(&m)), Some(WorkspaceRole::Editor));
        assert_eq!(effective_role(owner, other, Some(&m)), None);
        assert_eq!(effective_role(owner, other, None), None);
    }

    #[test]
    fn a_pending_invite_grants_nothing() {
        let (owner, user) = (ObjectId::new(), ObjectId::new());
        let pending = member(Some(user), WorkspaceRole::Editor, STATUS_PENDING);
        assert_eq!(effective_role(owner, user, Some(&pending)), None);
    }

    #[test]
    fn authorize_enforces_the_minimum_role() {
        assert!(authorize(Some(WorkspaceRole::Viewer), WorkspaceRole::Viewer).is_ok());
        assert!(authorize(Some(WorkspaceRole::Viewer), WorkspaceRole::Editor).is_err());
        assert!(authorize(Some(WorkspaceRole::Editor), WorkspaceRole::Owner).is_err());
        assert!(authorize(Some(WorkspaceRole::Owner), WorkspaceRole::Editor).is_ok());
        assert!(authorize(None, WorkspaceRole::Viewer).is_err());
    }

    #[test]
    fn invite_tokens_are_random_long_and_stored_only_as_a_hash() {
        let (a, hash_a) = new_invite_token();
        let (b, _) = new_invite_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_ne!(a, hash_a);
        assert_eq!(hash_invite_token(&a), hash_a);
    }
}
