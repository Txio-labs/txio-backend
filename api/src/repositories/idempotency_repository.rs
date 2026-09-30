use crate::utils::error::AppError;
use chrono::{DateTime, Utc};
use mongodb::bson::{doc, oid::ObjectId, DateTime as BsonDateTime};
use mongodb::options::IndexOptions;
use mongodb::{Collection as MongoCollection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const RETENTION_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IdempotencyRecord {
    pub user_id: ObjectId,
    pub key: String,
    /// SHA-256 of the request, so reusing a key for a different request is caught.
    pub request_hash: String,
    /// The transaction hash once the request has completed.
    pub result: Option<String>,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

pub enum Begin {
    /// First time this key is seen: the caller proceeds and must `complete` or `abort`.
    New,
    /// The same request already finished; return its stored result.
    Replay(String),
    /// Another request with this key is still running.
    InProgress,
    /// The key was used for a different request.
    Mismatch,
}

#[derive(Clone)]
pub struct IdempotencyRepository {
    collection: MongoCollection<IdempotencyRecord>,
}

impl IdempotencyRepository {
    pub fn new(db: &Database) -> Self {
        Self { collection: db.collection("idempotency_keys") }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let unique = IndexModel::builder()
            .keys(doc! { "user_id": 1, "key": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        let ttl = IndexModel::builder()
            .keys(doc! { "created_at": 1 })
            .options(IndexOptions::builder().expire_after(Duration::from_secs(RETENTION_SECONDS)).build())
            .build();
        self.collection.create_indexes(vec![unique, ttl], None).await?;
        Ok(())
    }

    /// Claims the key atomically via the unique index, so two concurrent
    /// requests with one key can never both proceed to broadcast.
    pub async fn begin(&self, user_id: ObjectId, key: &str, request_hash: &str) -> Result<Begin, AppError> {
        let record = IdempotencyRecord {
            user_id,
            key: key.to_string(),
            request_hash: request_hash.to_string(),
            result: None,
            created_at: Utc::now(),
        };
        match self.collection.insert_one(&record, None).await {
            Ok(_) => Ok(Begin::New),
            Err(e) if is_duplicate_key(&e) => {
                let existing = self
                    .collection
                    .find_one(doc! { "user_id": user_id, "key": key }, None)
                    .await?
                    .ok_or_else(|| AppError::InternalError("Idempotency record vanished".into()))?;
                Ok(if existing.request_hash != request_hash {
                    Begin::Mismatch
                } else {
                    match existing.result {
                        Some(result) => Begin::Replay(result),
                        None => Begin::InProgress,
                    }
                })
            }
            Err(e) => Err(AppError::Database(e)),
        }
    }

    pub async fn complete(&self, user_id: ObjectId, key: &str, result: &str) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "user_id": user_id, "key": key },
                doc! { "$set": { "result": result, "created_at": BsonDateTime::from_chrono(Utc::now()) } },
                None,
            )
            .await?;
        Ok(())
    }

    /// The request failed before anything was broadcast, so the key can be retried.
    pub async fn abort(&self, user_id: ObjectId, key: &str) -> Result<(), AppError> {
        self.collection.delete_one(doc! { "user_id": user_id, "key": key, "result": null }, None).await?;
        Ok(())
    }
}

fn is_duplicate_key(e: &mongodb::error::Error) -> bool {
    matches!(
        e.kind.as_ref(),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(w)) if w.code == 11000
    )
}
