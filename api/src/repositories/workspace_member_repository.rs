use crate::model::workspace_comment::WorkspaceComment;
use crate::model::workspace_member::{WorkspaceMember, WorkspaceRole, STATUS_ACTIVE, STATUS_PENDING};
use crate::utils::error::AppError;
use chrono::{DateTime, Utc};
use mongodb::bson::{doc, oid::ObjectId, Bson, DateTime as BsonDateTime};
use mongodb::options::{FindOptions, IndexOptions};
use mongodb::{Collection as MongoCollection, Database, IndexModel};

fn bson_time(t: DateTime<Utc>) -> BsonDateTime {
    BsonDateTime::from_chrono(t)
}

#[derive(Clone)]
pub struct WorkspaceMemberRepository {
    members: MongoCollection<WorkspaceMember>,
    comments: MongoCollection<WorkspaceComment>,
}

impl WorkspaceMemberRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            members: db.collection("workspace_members"),
            comments: db.collection("workspace_comments"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let unique = IndexModel::builder()
            .keys(doc! { "workspace_id": 1, "email": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        let by_user = IndexModel::builder().keys(doc! { "user_id": 1, "status": 1 }).build();
        let by_token = IndexModel::builder().keys(doc! { "token_hash": 1 }).build();
        // Only pending invites carry expires_at, so accepted members never expire.
        let ttl = IndexModel::builder()
            .keys(doc! { "expires_at": 1 })
            .options(IndexOptions::builder().expire_after(std::time::Duration::from_secs(0)).build())
            .build();
        self.members.create_indexes(vec![unique, by_user, by_token, ttl], None).await?;

        let by_target = IndexModel::builder()
            .keys(doc! { "workspace_id": 1, "target_id": 1, "created_at": 1 })
            .build();
        self.comments.create_index(by_target, None).await?;
        Ok(())
    }

    /// Creates a pending invite, or re-issues one (new token, new expiry, new
    /// role) if the address was already invited. An already-active member is left alone.
    pub async fn upsert_pending(
        &self,
        workspace_id: ObjectId,
        email: &str,
        role: WorkspaceRole,
        invited_by: ObjectId,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<WorkspaceMember, AppError> {
        if let Some(existing) = self.members.find_one(doc! { "workspace_id": workspace_id, "email": email }, None).await? {
            if existing.status == STATUS_ACTIVE {
                return Err(AppError::Conflict("This person is already a member of the workspace".into()));
            }
            self.members
                .update_one(
                    doc! { "_id": existing.id },
                    doc! { "$set": {
                        "role": mongodb::bson::to_bson(&role).map_err(|_| AppError::InternalError("role".into()))?,
                        "token_hash": token_hash,
                        "expires_at": bson_time(expires_at),
                        "invited_by": invited_by,
                    } },
                    None,
                )
                .await?;
            return self
                .members
                .find_one(doc! { "_id": existing.id }, None)
                .await?
                .ok_or_else(|| AppError::InternalError("Invite vanished".into()));
        }

        let member = WorkspaceMember {
            id: None,
            workspace_id,
            email: email.to_string(),
            user_id: None,
            role,
            status: STATUS_PENDING.to_string(),
            invited_by,
            token_hash: Some(token_hash.to_string()),
            expires_at: Some(expires_at),
            created_at: Utc::now(),
            accepted_at: None,
        };
        let result = self.members.insert_one(&member, None).await?;
        let mut created = member;
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn find_pending_by_token_hash(&self, token_hash: &str) -> Result<Option<WorkspaceMember>, AppError> {
        Ok(self
            .members
            .find_one(
                doc! { "token_hash": token_hash, "status": STATUS_PENDING, "expires_at": { "$gt": bson_time(Utc::now()) } },
                None,
            )
            .await?)
    }

    /// Turns a pending invite into a membership and burns the token (single use).
    pub async fn activate(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let result = self
            .members
            .update_one(
                doc! { "_id": id, "status": STATUS_PENDING },
                doc! {
                    "$set": { "status": STATUS_ACTIVE, "user_id": user_id, "accepted_at": bson_time(Utc::now()) },
                    "$unset": { "token_hash": Bson::Null, "expires_at": Bson::Null },
                },
                None,
            )
            .await?;
        if result.matched_count == 0 {
            return Err(AppError::NotFound("Invitation is no longer valid".into()));
        }
        Ok(())
    }

    pub async fn find_active(&self, workspace_id: ObjectId, user_id: ObjectId) -> Result<Option<WorkspaceMember>, AppError> {
        Ok(self
            .members
            .find_one(doc! { "workspace_id": workspace_id, "user_id": user_id, "status": STATUS_ACTIVE }, None)
            .await?)
    }

    pub async fn list_active_for_user(&self, user_id: ObjectId) -> Result<Vec<WorkspaceMember>, AppError> {
        let mut cursor = self.members.find(doc! { "user_id": user_id, "status": STATUS_ACTIVE }, None).await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub async fn list_for_workspace(&self, workspace_id: ObjectId) -> Result<Vec<WorkspaceMember>, AppError> {
        let options = FindOptions::builder().sort(doc! { "created_at": 1 }).build();
        let mut cursor = self.members.find(doc! { "workspace_id": workspace_id }, options).await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub async fn find_in_workspace(&self, workspace_id: ObjectId, member_id: ObjectId) -> Result<WorkspaceMember, AppError> {
        self.members
            .find_one(doc! { "_id": member_id, "workspace_id": workspace_id }, None)
            .await?
            .ok_or_else(|| AppError::NotFound("Member not found".into()))
    }

    pub async fn set_role(&self, member_id: ObjectId, role: WorkspaceRole) -> Result<(), AppError> {
        self.members
            .update_one(
                doc! { "_id": member_id },
                doc! { "$set": { "role": mongodb::bson::to_bson(&role).map_err(|_| AppError::InternalError("role".into()))? } },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn delete(&self, member_id: ObjectId) -> Result<(), AppError> {
        self.members.delete_one(doc! { "_id": member_id }, None).await?;
        Ok(())
    }

    /// Removing a workspace removes its memberships and comments with it.
    pub async fn delete_all_for_workspace(&self, workspace_id: ObjectId) -> Result<(), AppError> {
        self.members.delete_many(doc! { "workspace_id": workspace_id }, None).await?;
        self.comments.delete_many(doc! { "workspace_id": workspace_id }, None).await?;
        Ok(())
    }

    // --- Comments ---

    pub async fn add_comment(&self, comment: &WorkspaceComment) -> Result<WorkspaceComment, AppError> {
        let result = self.comments.insert_one(comment, None).await?;
        let mut created = comment.clone();
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn list_comments(&self, workspace_id: ObjectId, target_id: &str) -> Result<Vec<WorkspaceComment>, AppError> {
        let options = FindOptions::builder().sort(doc! { "created_at": 1 }).limit(500).build();
        let mut cursor = self
            .comments
            .find(doc! { "workspace_id": workspace_id, "target_id": target_id }, options)
            .await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub async fn find_comment(&self, workspace_id: ObjectId, comment_id: ObjectId) -> Result<WorkspaceComment, AppError> {
        self.comments
            .find_one(doc! { "_id": comment_id, "workspace_id": workspace_id }, None)
            .await?
            .ok_or_else(|| AppError::NotFound("Comment not found".into()))
    }

    pub async fn delete_comment(&self, comment_id: ObjectId) -> Result<(), AppError> {
        self.comments.delete_one(doc! { "_id": comment_id }, None).await?;
        Ok(())
    }
}
