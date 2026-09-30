use crate::model::history::HistoryEntry;
use crate::model::workspace_member::WorkspaceRole;
use crate::repositories::history_repository::HistoryRepository;
use crate::services::workspace_access::WorkspaceAccess;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;
use serde_json::Value;

/// History stays per-user even in a shared workspace: a member sees their own
/// runs, never a colleague's. Membership only decides who may use the workspace.
#[derive(Clone)]
pub struct HistoryService {
    history_repo: HistoryRepository,
    access: WorkspaceAccess,
}

impl HistoryService {
    pub fn new(history_repo: HistoryRepository, access: WorkspaceAccess) -> Self {
        Self { history_repo, access }
    }

    async fn ensure_workspace_owner(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(), AppError> {
        self.access.require(workspace_id, user_id, WorkspaceRole::Viewer).await?;
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
        wallet_family: Option<String>,
        wallet_address: Option<String>,
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
            wallet_family,
            wallet_address,
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
        wallet_address: Option<String>,
        wallet_family: Option<String>,
        chain: Option<String>,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        if let Some(ws) = workspace_id {
            self.ensure_workspace_owner(ws, user_id).await?;
        }

        if wallet_address.is_some() || wallet_family.is_some() || chain.is_some() {
            return self
                .history_repo
                .find_by_user_and_wallet(user_id, workspace_id, wallet_address, wallet_family, chain)
                .await;
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
