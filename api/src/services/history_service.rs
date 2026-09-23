use crate::model::history::HistoryEntry;
use crate::repositories::{
    history_repository::HistoryRepository, workspace_repository::WorkspaceRepository,
};
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;
use serde_json::Value;

#[derive(Clone)]
pub struct HistoryService {
    history_repo: HistoryRepository,
    workspace_repo: WorkspaceRepository,
}

impl HistoryService {
    pub fn new(history_repo: HistoryRepository, workspace_repo: WorkspaceRepository) -> Self {
        Self {
            history_repo,
            workspace_repo,
        }
    }

    async fn ensure_workspace_owner(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(), AppError> {
        let workspace = self.workspace_repo.find_by_id(workspace_id).await?;

        if workspace.user_id != user_id {
            return Err(AppError::Forbidden(
                "Not authorized to access this workspace".into(),
            ));
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
        name: String,
        request_type: String,
        chain: Option<String>,
        network: String,
        method: Option<String>,
        params: Option<Value>,
        tx_params: Option<Value>,
        result_data: Option<Value>,
        status: i32,
        duration_ms: i64,
    ) -> Result<HistoryEntry, AppError> {
        if let Some(ws) = workspace_id {
            self.ensure_workspace_owner(ws, user_id).await?;
        }

        let entry = HistoryEntry::new(
            user_id,
            workspace_id,
            name,
            request_type,
            chain,
            network,
            method,
            params,
            tx_params,
            result_data,
            status,
            duration_ms,
        );

        self.history_repo.insert(&entry).await
    }

    pub async fn list(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        if let Some(ws) = workspace_id {
            self.ensure_workspace_owner(ws, user_id).await?;
        }

        self.history_repo.find_by_user(user_id, workspace_id).await
    }

    pub async fn clear(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
    ) -> Result<(), AppError> {
        if let Some(ws) = workspace_id {
            self.ensure_workspace_owner(ws, user_id).await?;
        }

        self.history_repo
            .delete_all_by_user(user_id, workspace_id)
            .await
    }

    pub async fn delete_one(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.history_repo.delete_one(id, user_id).await
    }
}
