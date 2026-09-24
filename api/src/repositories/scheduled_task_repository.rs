use crate::model::scheduled_task::{ScheduledTask, ScheduledTaskStatus};
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::FindOptions;
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct ScheduledTaskRepository {
    collection: MongoCollection<ScheduledTask>,
}

impl ScheduledTaskRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("scheduled_tasks"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let by_user = IndexModel::builder().keys(doc! { "user_id": 1 }).build();
        self.collection.create_index(by_user, None).await?;

        // The worker's poll loop scans for due, active tasks — index the
        // fields that filter narrows on.
        let due_index = IndexModel::builder()
            .keys(doc! { "status": 1, "next_run_at": 1 })
            .build();
        self.collection.create_index(due_index, None).await?;

        Ok(())
    }

    pub async fn insert(&self, task: &ScheduledTask) -> Result<ScheduledTask, AppError> {
        let result = self.collection.insert_one(task, None).await?;
        let mut created = task.clone();
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn find_by_user(&self, user_id: ObjectId) -> Result<Vec<ScheduledTask>, AppError> {
        let find_options = FindOptions::builder().sort(doc! { "created_at": -1 }).build();
        let mut cursor = self
            .collection
            .find(doc! { "user_id": user_id }, find_options)
            .await?;
        let mut tasks = Vec::new();
        while cursor.advance().await? {
            tasks.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(tasks)
    }

    /// Active tasks with a fixed `next_run_at` that has passed — used by the
    /// worker for Recurring/TimeOnce triggers. PriceThreshold tasks have no
    /// `next_run_at` and are polled separately by the worker (every tick,
    /// since they're condition-based rather than time-based).
    pub async fn find_due(&self) -> Result<Vec<ScheduledTask>, AppError> {
        let filter = doc! {
            "status": "active",
            "next_run_at": { "$ne": null, "$lte": mongodb::bson::DateTime::from_chrono(Utc::now()) },
        };
        let mut cursor = self.collection.find(filter, None).await?;
        let mut tasks = Vec::new();
        while cursor.advance().await? {
            tasks.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(tasks)
    }

    pub async fn find_active_price_triggered(&self) -> Result<Vec<ScheduledTask>, AppError> {
        let filter = doc! { "status": "active", "trigger.kind": "price_threshold" };
        let mut cursor = self.collection.find(filter, None).await?;
        let mut tasks = Vec::new();
        while cursor.advance().await? {
            tasks.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(tasks)
    }

    pub async fn mark_ran(
        &self,
        id: ObjectId,
        next_run_at: Option<chrono::DateTime<Utc>>,
        error: Option<String>,
    ) -> Result<(), AppError> {
        let mut set_doc = doc! {
            "last_run_at": mongodb::bson::DateTime::from_chrono(Utc::now()),
        };
        if let Some(next) = next_run_at {
            set_doc.insert("next_run_at", mongodb::bson::DateTime::from_chrono(next));
        }
        set_doc.insert("last_error", error);

        self.collection
            .update_one(doc! { "_id": id }, doc! { "$set": set_doc }, None)
            .await?;
        Ok(())
    }

    pub async fn set_status(
        &self,
        id: ObjectId,
        user_id: ObjectId,
        status: ScheduledTaskStatus,
    ) -> Result<(), AppError> {
        let status_str = match status {
            ScheduledTaskStatus::Active => "active",
            ScheduledTaskStatus::Paused => "paused",
            ScheduledTaskStatus::Cancelled => "cancelled",
        };
        let result = self
            .collection
            .update_one(
                doc! { "_id": id, "user_id": user_id },
                doc! { "$set": { "status": status_str } },
                None,
            )
            .await?;

        if result.matched_count == 0 {
            return Err(AppError::NotFound(format!("Scheduled task not found: {id}")));
        }
        Ok(())
    }
}
