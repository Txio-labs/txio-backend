use crate::model::webhook_subscription::WebhookSubscription;
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct WebhookSubscriptionRepository {
    collection: MongoCollection<WebhookSubscription>,
}

impl WebhookSubscriptionRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("webhook_subscriptions"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let index = IndexModel::builder().keys(doc! { "user_id": 1 }).build();
        self.collection.create_index(index, None).await?;
        Ok(())
    }

    pub async fn insert(&self, sub: &WebhookSubscription) -> Result<WebhookSubscription, AppError> {
        let result = self.collection.insert_one(sub, None).await?;
        let mut created = sub.clone();
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn find_by_user(&self, user_id: ObjectId) -> Result<Vec<WebhookSubscription>, AppError> {
        let mut cursor = self.collection.find(doc! { "user_id": user_id }, None).await?;
        let mut subs = Vec::new();
        while cursor.advance().await? {
            subs.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(subs)
    }

    /// Every active subscription across all users listening for a given
    /// event — the delivery worker's fan-out query.
    pub async fn find_active_for_event(&self, event: &str) -> Result<Vec<WebhookSubscription>, AppError> {
        let mut cursor = self
            .collection
            .find(doc! { "is_active": true, "events": event }, None)
            .await?;
        let mut subs = Vec::new();
        while cursor.advance().await? {
            subs.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(subs)
    }

    pub async fn delete(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let result = self
            .collection
            .delete_one(doc! { "_id": id, "user_id": user_id }, None)
            .await?;
        if result.deleted_count == 0 {
            return Err(AppError::NotFound(format!("Webhook subscription not found: {id}")));
        }
        Ok(())
    }

    pub async fn record_delivery(
        &self,
        id: ObjectId,
        error: Option<String>,
    ) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": {
                    "last_delivered_at": mongodb::bson::DateTime::from_chrono(Utc::now()),
                    "last_delivery_error": error,
                } },
                None,
            )
            .await?;
        Ok(())
    }
}
