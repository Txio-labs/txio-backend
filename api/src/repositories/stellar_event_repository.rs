use crate::model::stellar_event::{StellarContractEvent, StellarContractWatch, StellarTokenTransfer};
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, Document};
use mongodb::options::{FindOptions, UpdateOptions};
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct StellarEventRepository {
    events: MongoCollection<StellarContractEvent>,
    watches: MongoCollection<StellarContractWatch>,
    transfers: MongoCollection<StellarTokenTransfer>,
}

impl StellarEventRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            events: db.collection("stellar_contract_events"),
            watches: db.collection("stellar_contract_watch"),
            transfers: db.collection("stellar_token_transfers"),
        }
    }

    pub async fn ensure_indexes(&self) -> Result<(), AppError> {
        let event_index = IndexModel::builder()
            .keys(doc! { "network": 1, "contract_id": 1, "ledger": -1 })
            .build();
        self.events.create_index(event_index, None).await?;

        let watch_index = IndexModel::builder()
            .keys(doc! { "network": 1, "contract_id": 1 })
            .options(
                mongodb::options::IndexOptions::builder()
                    .unique(true)
                    .build(),
            )
            .build();
        self.watches.create_index(watch_index, None).await?;

        for field in ["from", "to"] {
            let index = IndexModel::builder()
                .keys(doc! { "network": 1, field: 1 })
                .build();
            self.transfers.create_index(index, None).await?;
        }
        Ok(())
    }

    /// Idempotent: re-reading a ledger range never duplicates an event.
    pub async fn upsert_event(&self, event: &StellarContractEvent) -> Result<(), AppError> {
        let set_on_insert = mongodb::bson::to_document(event)
            .map_err(|e| AppError::InternalError(e.to_string()))?;
        let update: Document = doc! { "$setOnInsert": set_on_insert };
        let options = UpdateOptions::builder().upsert(true).build();
        self.events
            .update_one(doc! { "_id": &event.id }, update, options)
            .await?;
        Ok(())
    }

    pub async fn recent_events(
        &self,
        network: &str,
        contract_id: &str,
        limit: i64,
    ) -> Result<Vec<StellarContractEvent>, AppError> {
        let options = FindOptions::builder()
            .sort(doc! { "ledger": -1, "_id": -1 })
            .limit(limit)
            .build();
        let mut cursor = self
            .events
            .find(doc! { "network": network, "contract_id": contract_id }, options)
            .await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    /// Registers a contract for indexing. A no-op when it is already tracked.
    pub async fn ensure_watch(&self, network: &str, contract_id: &str) -> Result<StellarContractWatch, AppError> {
        let key = format!("{network}:{contract_id}");
        let on_insert = doc! {
            "network": network,
            "contract_id": contract_id,
            "backfilled": false,
            "last_ledger": 0_i64,
            "added_at": mongodb::bson::DateTime::from_chrono(Utc::now()),
        };
        let options = UpdateOptions::builder().upsert(true).build();
        self.watches
            .update_one(doc! { "_id": &key }, doc! { "$setOnInsert": on_insert }, options)
            .await?;
        self.watches
            .find_one(doc! { "_id": &key }, None)
            .await?
            .ok_or_else(|| AppError::InternalError("watch missing after upsert".into()))
    }

    pub async fn all_watches(&self) -> Result<Vec<StellarContractWatch>, AppError> {
        let mut cursor = self.watches.find(doc! {}, None).await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub async fn set_watch_progress(
        &self,
        key: &str,
        last_ledger: i64,
        backfilled: bool,
    ) -> Result<(), AppError> {
        self.watches
            .update_one(
                doc! { "_id": key },
                doc! { "$set": { "last_ledger": last_ledger, "backfilled": backfilled } },
                None,
            )
            .await?;
        Ok(())
    }

    /// Idempotent on the source event id.
    pub async fn upsert_transfer(&self, transfer: &StellarTokenTransfer) -> Result<(), AppError> {
        let set_on_insert = mongodb::bson::to_document(transfer)
            .map_err(|e| AppError::InternalError(e.to_string()))?;
        let options = UpdateOptions::builder().upsert(true).build();
        self.transfers
            .update_one(doc! { "_id": &transfer.id }, doc! { "$setOnInsert": set_on_insert }, options)
            .await?;
        Ok(())
    }

    /// Every indexed transfer where `holder` sent or received, on one network.
    pub async fn transfers_for_holder(
        &self,
        network: &str,
        holder: &str,
    ) -> Result<Vec<StellarTokenTransfer>, AppError> {
        let filter = doc! {
            "network": network,
            "$or": [{ "from": holder }, { "to": holder }],
        };
        let mut cursor = self.transfers.find(filter, None).await?;
        let mut out = Vec::new();
        while cursor.advance().await? {
            out.push(cursor.deserialize_current().map_err(AppError::Database)?);
        }
        Ok(out)
    }
}
