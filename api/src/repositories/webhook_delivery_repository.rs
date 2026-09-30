use crate::model::webhook_delivery::{WebhookDelivery, STATUS_DEAD, STATUS_DELIVERED, STATUS_IN_FLIGHT, STATUS_PENDING};
use crate::utils::error::AppError;
use chrono::{DateTime, Duration, Utc};
use mongodb::bson::{doc, oid::ObjectId, DateTime as BsonDateTime};
use mongodb::options::{FindOneAndUpdateOptions, FindOptions, IndexOptions, ReturnDocument};
use mongodb::{Collection as MongoCollection, Database, IndexModel};
use std::time::Duration as StdDuration;

/// Delivery records are an operational log, not an archive.
const RETENTION_SECONDS: u64 = 14 * 24 * 60 * 60;

fn bson_time(t: DateTime<Utc>) -> BsonDateTime {
    BsonDateTime::from_chrono(t)
}

#[derive(Clone)]
pub struct WebhookDeliveryRepository {
    collection: MongoCollection<WebhookDelivery>,
}

impl WebhookDeliveryRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("webhook_deliveries"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let due = IndexModel::builder()
            .keys(doc! { "status": 1, "next_attempt_at": 1 })
            .build();
        let by_sub = IndexModel::builder()
            .keys(doc! { "subscription_id": 1, "created_at": -1 })
            .build();
        let ttl = IndexModel::builder()
            .keys(doc! { "created_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("webhook_deliveries_ttl".to_string()))
                    .expire_after(StdDuration::from_secs(RETENTION_SECONDS))
                    .build(),
            )
            .build();
        self.collection.create_indexes(vec![due, by_sub, ttl], None).await?;
        Ok(())
    }

    pub async fn insert(&self, delivery: &WebhookDelivery) -> Result<(), AppError> {
        self.collection.insert_one(delivery, None).await?;
        Ok(())
    }

    /// Atomically takes one due delivery and leases it: it is marked in
    /// flight and its next attempt is pushed out by `lease`, so a crashed
    /// worker's delivery becomes due again instead of being lost, and two
    /// workers never take the same one.
    pub async fn claim_due(&self, lease: Duration) -> Result<Option<WebhookDelivery>, AppError> {
        let now = Utc::now();
        let filter = doc! {
            "status": { "$in": [STATUS_PENDING, STATUS_IN_FLIGHT] },
            "next_attempt_at": { "$lte": bson_time(now) },
        };
        let update = doc! {
            "$set": { "status": STATUS_IN_FLIGHT, "next_attempt_at": bson_time(now + lease) },
            "$inc": { "attempts": 1 },
        };
        let options = FindOneAndUpdateOptions::builder()
            .sort(doc! { "next_attempt_at": 1 })
            .return_document(ReturnDocument::After)
            .build();
        Ok(self.collection.find_one_and_update(filter, update, options).await?)
    }

    pub async fn mark_delivered(&self, id: ObjectId, status_code: u16) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": {
                    "status": STATUS_DELIVERED,
                    "last_status_code": status_code as i32,
                    "last_error": mongodb::bson::Bson::Null,
                    "delivered_at": bson_time(Utc::now()),
                } },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn mark_retry(
        &self,
        id: ObjectId,
        next_attempt_at: DateTime<Utc>,
        status_code: Option<u16>,
        error: String,
    ) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": {
                    "status": STATUS_PENDING,
                    "next_attempt_at": bson_time(next_attempt_at),
                    "last_status_code": status_code.map(|c| c as i32),
                    "last_error": error,
                } },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn mark_dead(&self, id: ObjectId, status_code: Option<u16>, error: String) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": {
                    "status": STATUS_DEAD,
                    "last_status_code": status_code.map(|c| c as i32),
                    "last_error": error,
                } },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn list_for_subscription(
        &self,
        subscription_id: ObjectId,
        user_id: ObjectId,
        limit: i64,
    ) -> Result<Vec<WebhookDelivery>, AppError> {
        let options = FindOptions::builder()
            .sort(doc! { "created_at": -1 })
            .limit(limit)
            .build();
        let mut cursor = self
            .collection
            .find(doc! { "subscription_id": subscription_id, "user_id": user_id }, options)
            .await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    /// Puts a finished delivery back in the queue (manual redelivery). The
    /// delivery id is kept, so receivers can still dedupe.
    pub async fn requeue(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let result = self
            .collection
            .update_one(
                doc! { "_id": id, "user_id": user_id, "status": { "$in": [STATUS_DELIVERED, STATUS_DEAD] } },
                doc! { "$set": {
                    "status": STATUS_PENDING,
                    "attempts": 0,
                    "next_attempt_at": bson_time(Utc::now()),
                    "last_error": mongodb::bson::Bson::Null,
                } },
                None,
            )
            .await?;
        if result.matched_count == 0 {
            return Err(AppError::NotFound("Delivery not found or still in progress".into()));
        }
        Ok(())
    }
}
