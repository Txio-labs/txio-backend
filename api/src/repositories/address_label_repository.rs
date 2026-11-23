use crate::model::address_label::AddressLabel;
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::{FindOptions, IndexOptions, UpdateOptions};
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct AddressLabelRepository {
    collection: MongoCollection<AddressLabel>,
}

impl AddressLabelRepository {
    pub fn new(db: &Database) -> Self {
        Self { collection: db.collection("address_labels") }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let unique = IndexModel::builder()
            .keys(doc! { "user_id": 1, "chain": 1, "address": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        self.collection.create_index(unique, None).await?;
        Ok(())
    }

    /// Creates or replaces the caller's label for one address on one chain.
    pub async fn upsert(&self, user_id: ObjectId, chain: &str, address: &str, label: &str) -> Result<(), AppError> {
        let filter = doc! { "user_id": user_id, "chain": chain, "address": address };
        let update = doc! {
            "$set": { "label": label, "updated_at": mongodb::bson::DateTime::from_chrono(Utc::now()) },
        };
        self.collection
            .update_one(filter, update, UpdateOptions::builder().upsert(true).build())
            .await?;
        Ok(())
    }

    /// The caller's labels for one address, optionally limited to one chain.
    pub async fn find_by_address(
        &self,
        user_id: ObjectId,
        address: &str,
        chain: Option<&str>,
    ) -> Result<Vec<AddressLabel>, AppError> {
        let mut filter = doc! { "user_id": user_id, "address": address };
        if let Some(chain) = chain {
            filter.insert("chain", chain);
        }
        let options = FindOptions::builder().sort(doc! { "chain": 1 }).build();
        let mut cursor = self.collection.find(filter, options).await?;
        let mut labels = Vec::new();
        while cursor.advance().await? {
            labels.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(labels)
    }
}
