use crate::model::history::HistoryEntry;
use crate::utils::error::AppError;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::FindOptions;
use mongodb::{Collection as MongoCollection, Database};

/// Hard cap on history entries kept per user — history is an audit trail,
/// not permanent storage, so it's pruned like the in-memory terminal
/// execution log (terminal_service.rs) rather than left to grow forever.
const MAX_ENTRIES_PER_USER: i64 = 500;

#[derive(Clone)]
pub struct HistoryRepository {
    collection: MongoCollection<HistoryEntry>,
}

impl HistoryRepository {
    pub fn new(db: &Database) -> Self {
        let collection = db.collection("history");
        Self { collection }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        use mongodb::IndexModel;

        let index = IndexModel::builder()
            .keys(doc! { "user_id": 1, "executed_at": -1 })
            .build();
        self.collection.create_index(index, None).await?;

        let wallet_index = IndexModel::builder()
            .keys(doc! { "user_id": 1, "wallet_address": 1, "executed_at": -1 })
            .build();
        self.collection.create_index(wallet_index, None).await?;

        Ok(())
    }

    pub async fn insert(&self, entry: &HistoryEntry) -> Result<HistoryEntry, AppError> {
        let result = self.collection.insert_one(entry, None).await?;
        let mut created = entry.clone();
        created.id = result.inserted_id.as_object_id();

        self.prune(entry.user_id).await?;

        Ok(created)
    }

    /// Keeps only the most recent `MAX_ENTRIES_PER_USER` entries for a user,
    /// deleting anything older. Called after every insert so the collection
    /// never grows unbounded for an active user.
    async fn prune(&self, user_id: ObjectId) -> Result<(), AppError> {
        let filter = doc! { "user_id": user_id };
        let find_options = FindOptions::builder()
            .sort(doc! { "executed_at": -1 })
            .skip(MAX_ENTRIES_PER_USER as u64)
            .build();

        let mut cursor = self.collection.find(filter, find_options).await?;
        let mut stale_ids = Vec::new();
        while cursor.advance().await? {
            let entry: HistoryEntry = cursor.deserialize_current().map_err(AppError::Database)?;
            if let Some(id) = entry.id {
                stale_ids.push(id);
            }
        }

        if !stale_ids.is_empty() {
            self.collection
                .delete_many(doc! { "_id": { "$in": stale_ids } }, None)
                .await?;
        }

        Ok(())
    }

    pub async fn find_by_user(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        let mut filter = doc! { "user_id": user_id };
        if let Some(ws) = workspace_id {
            filter.insert("workspace_id", ws);
        }

        let find_options = FindOptions::builder()
            .sort(doc! { "executed_at": -1 })
            .limit(MAX_ENTRIES_PER_USER)
            .build();

        let mut cursor = self.collection.find(filter, find_options).await?;
        let mut entries = Vec::new();
        while cursor.advance().await? {
            let entry: HistoryEntry = cursor.deserialize_current().map_err(AppError::Database)?;
            entries.push(entry);
        }

        Ok(entries)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn find_by_user_and_wallet(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
        wallet_address: Option<String>,
        wallet_family: Option<String>,
        chain: Option<String>,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        let mut filter = doc! { "user_id": user_id };
        if let Some(ws) = workspace_id {
            filter.insert("workspace_id", ws);
        }
        if let Some(address) = wallet_address {
            filter.insert("wallet_address", address);
        }
        if let Some(family) = wallet_family {
            filter.insert("wallet_family", family);
        }
        if let Some(chain) = chain {
            filter.insert("chain", chain);
        }

        let find_options = FindOptions::builder()
            .sort(doc! { "executed_at": -1 })
            .limit(MAX_ENTRIES_PER_USER)
            .build();

        let mut cursor = self.collection.find(filter, find_options).await?;
        let mut entries = Vec::new();
        while cursor.advance().await? {
            let entry: HistoryEntry = cursor.deserialize_current().map_err(AppError::Database)?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub async fn delete_all_by_user(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
    ) -> Result<(), AppError> {
        let mut filter = doc! { "user_id": user_id };
        if let Some(ws) = workspace_id {
            filter.insert("workspace_id", ws);
        }

        self.collection.delete_many(filter, None).await?;
        Ok(())
    }

    pub async fn delete_one(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let filter = doc! { "_id": id, "user_id": user_id };
        let result = self.collection.delete_one(filter, None).await?;

        if result.deleted_count == 0 {
            return Err(AppError::NotFound(format!(
                "History entry not found with id: {id}"
            )));
        }
        Ok(())
    }
}
