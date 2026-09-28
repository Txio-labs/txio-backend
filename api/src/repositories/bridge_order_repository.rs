use chrono::Utc;
use mongodb::{bson::doc, Collection, Database};

use crate::{
    model::bridge_order::{BridgeOrder, BridgeOrderStatus},
    utils::error::AppError,
};

#[derive(Clone)]
pub struct BridgeOrderRepository {
    collection: Collection<BridgeOrder>,
}

impl BridgeOrderRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("bridge_orders"),
        }
    }

    pub async fn ensure_indices(&self) -> mongodb::error::Result<()> {
        use mongodb::{options::IndexOptions, IndexModel};

        let provider_order_id_index = IndexModel::builder()
            .keys(doc! { "provider_order_id": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        let user_id_index = IndexModel::builder().keys(doc! { "user_id": 1 }).build();

        self.collection
            .create_indexes([provider_order_id_index, user_id_index], None)
            .await?;
        Ok(())
    }

    pub async fn insert(&self, order: &BridgeOrder) -> Result<(), AppError> {
        self.collection.insert_one(order, None).await?;
        Ok(())
    }

    pub async fn find_by_provider_order_id(
        &self,
        provider_order_id: &str,
    ) -> Result<Option<BridgeOrder>, AppError> {
        Ok(self
            .collection
            .find_one(doc! { "provider_order_id": provider_order_id }, None)
            .await?)
    }

    pub async fn update_status(
        &self,
        provider_order_id: &str,
        status: BridgeOrderStatus,
        tx_hash: Option<String>,
    ) -> Result<(), AppError> {
        let status_bson = mongodb::bson::to_bson(&status)
            .map_err(|e| AppError::InternalError(e.to_string()))?;

        self.collection
            .update_one(
                doc! { "provider_order_id": provider_order_id },
                doc! {
                    "$set": {
                        "status": status_bson,
                        "tx_hash": tx_hash,
                        "updated_at": mongodb::bson::DateTime::from_chrono(Utc::now()),
                    }
                },
                None,
            )
            .await?;

        Ok(())
    }
}
