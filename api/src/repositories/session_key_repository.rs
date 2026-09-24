use crate::model::session_key::SessionKey;
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::FindOptions;
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct SessionKeyRepository {
    collection: MongoCollection<SessionKey>,
}

impl SessionKeyRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("session_keys"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let index = IndexModel::builder()
            .keys(doc! { "user_id": 1, "wallet_address": 1 })
            .build();
        self.collection.create_index(index, None).await?;
        Ok(())
    }

    pub async fn insert(&self, key: &SessionKey) -> Result<SessionKey, AppError> {
        let result = self.collection.insert_one(key, None).await?;
        let mut created = key.clone();
        created.id = result.inserted_id.as_object_id();
        Ok(created)
    }

    pub async fn find_by_user(&self, user_id: ObjectId) -> Result<Vec<SessionKey>, AppError> {
        let find_options = FindOptions::builder().sort(doc! { "created_at": -1 }).build();
        let mut cursor = self
            .collection
            .find(doc! { "user_id": user_id }, find_options)
            .await?;
        let mut keys = Vec::new();
        while cursor.advance().await? {
            keys.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(keys)
    }

    pub async fn find_by_id(&self, id: ObjectId, user_id: ObjectId) -> Result<SessionKey, AppError> {
        self.collection
            .find_one(doc! { "_id": id, "user_id": user_id }, None)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("Session key not found: {id}")))
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
            return Err(AppError::NotFound(format!("Session key not found: {id}")));
        }
        Ok(())
    }
}
