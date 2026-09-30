use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// A comment on a request (or collection) in a shared workspace, visible to
/// every member of that workspace and to nobody else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceComment {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub workspace_id: ObjectId,
    /// The request or collection id the comment is attached to.
    pub target_id: String,
    pub author_id: ObjectId,
    pub author_email: String,
    pub body: String,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}
