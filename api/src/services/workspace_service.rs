use crate::model::workspace::{Workspace, WorkspaceType};
use crate::repositories::{
    collection_repository::CollectionRepository, history_repository::HistoryRepository,
    request_repository::RequestRepository, workspace_repository::WorkspaceRepository,
};
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;

#[derive(Clone)]
pub struct WorkspaceService {
    workspace_repo: WorkspaceRepository,
    collection_repo: CollectionRepository,
    request_repo: RequestRepository,
    history_repo: HistoryRepository,
}

impl WorkspaceService {
    pub fn new(
        workspace_repo: WorkspaceRepository,
        collection_repo: CollectionRepository,
        request_repo: RequestRepository,
        history_repo: HistoryRepository,
    ) -> Self {
        Self {
            workspace_repo,
            collection_repo,
            request_repo,
            history_repo,
        }
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

    pub async fn get_user_workspaces(&self, user_id: ObjectId) -> Result<Vec<Workspace>, AppError> {
        self.workspace_repo.find_all_by_user(user_id).await
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

        self.workspace_repo.delete(workspace_id).await?;

        Ok(())
    }
}
