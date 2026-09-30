use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// What someone may do in a workspace. Declaration order is the order of
/// privilege, so `role >= WorkspaceRole::Editor` reads as "can edit".
///
/// The owner is not stored as a member: it is always `Workspace.user_id`, so
/// workspaces created before memberships existed need no migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    /// Read collections and requests, simulate, comment.
    Viewer,
    /// Everything a viewer can, plus create, edit and delete collections and requests.
    Editor,
    /// Everything, plus manage members and rename or delete the workspace.
    Owner,
}

impl WorkspaceRole {
    pub fn can_edit(self) -> bool {
        self >= WorkspaceRole::Editor
    }

    /// Roles an invitation may grant. Ownership is not transferable through invites.
    pub fn parse_invitable(value: &str) -> Option<WorkspaceRole> {
        match value {
            "viewer" => Some(WorkspaceRole::Viewer),
            "editor" => Some(WorkspaceRole::Editor),
            _ => None,
        }
    }
}

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_ACTIVE: &str = "active";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub workspace_id: ObjectId,
    /// Lowercased. A pending invite is addressed by email; the account is
    /// linked when the invite is accepted.
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<ObjectId>,
    /// Only `Viewer` or `Editor`.
    pub role: WorkspaceRole,
    /// `pending` until accepted, then `active`.
    pub status: String,
    pub invited_by: ObjectId,
    /// SHA-256 of the invite token; the token itself is only ever in the email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
    /// Set on pending invites only; a TTL index removes them after this time.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub accepted_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered_by_privilege() {
        assert!(WorkspaceRole::Owner > WorkspaceRole::Editor);
        assert!(WorkspaceRole::Editor > WorkspaceRole::Viewer);
        assert!(WorkspaceRole::Editor.can_edit());
        assert!(!WorkspaceRole::Viewer.can_edit());
    }

    #[test]
    fn owner_cannot_be_granted_by_invite() {
        assert_eq!(WorkspaceRole::parse_invitable("editor"), Some(WorkspaceRole::Editor));
        assert_eq!(WorkspaceRole::parse_invitable("viewer"), Some(WorkspaceRole::Viewer));
        assert_eq!(WorkspaceRole::parse_invitable("owner"), None);
        assert_eq!(WorkspaceRole::parse_invitable("admin"), None);
    }
}
