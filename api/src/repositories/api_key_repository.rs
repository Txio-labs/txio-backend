use crate::model::api_key::ApiKey;
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::FindOptions;
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct ApiKeyRepository {
    collection: MongoCollection<ApiKey>,
}

impl ApiKeyRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("api_keys"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let by_user = IndexModel::builder().keys(doc! { "user_id": 1 }).build();
        self.collection.create_index(by_user, None).await?;

        // Looked up on every public-API request by hash — must be indexed
        // for auth to stay fast as the collection grows.
        let by_hash = IndexModel::builder()
            .keys(doc! { "key_hash": 1 })
            .options(mongodb::options::IndexOptions::builder().unique(true).build())
            .build();
        self.collection.create_index(by_hash, None).await?;

        Ok(())
    }

    pub async fn insert(&self, key: &ApiKey) -> Result<ApiKey, AppError> {
        let result = self.collection.insert_one(key, None).await?;
        let mut created = key.clone();
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn find_by_user(&self, user_id: ObjectId) -> Result<Vec<ApiKey>, AppError> {
        let find_options = FindOptions::builder().sort(doc! { "created_at": -1 }).build();
        let mut cursor = self.collection.find(doc! { "user_id": user_id }, find_options).await?;
        let mut keys = Vec::new();
        while cursor.advance().await? {
            keys.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(keys)
    }

    /// The hot path: resolves a hashed key to its record, used by
    /// `ApiKeyAuth` on every public API request.
    pub async fn find_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, AppError> {
        Ok(self.collection.find_one(doc! { "key_hash": key_hash }, None).await?)
    }

    pub async fn revoke(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        let result = self
            .collection
            .update_one(
                doc! { "_id": id, "user_id": user_id },
                doc! { "$set": { "revoked_at": mongodb::bson::DateTime::from_chrono(Utc::now()) } },
                None,
            )
            .await?;
        if result.matched_count == 0 {
            return Err(AppError::NotFound(format!("API key not found: {id}")));
        }
        Ok(())
    }

    /// Fire-and-forget usage stamp — never blocks the request it's
    /// piggybacking on, errors are swallowed at the call site.
    pub async fn touch_last_used(&self, id: ObjectId) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": { "last_used_at": mongodb::bson::DateTime::from_chrono(Utc::now()) } },
                None,
            )
            .await?;
        Ok(())
    }
}
