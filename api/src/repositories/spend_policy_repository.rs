use crate::model::spend_policy::{SpendPolicy, SpendUsage};
use crate::utils::error::AppError;
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use mongodb::options::{FindOneAndUpdateOptions, IndexOptions, ReturnDocument};
use mongodb::{Collection as MongoCollection, Database, IndexModel};

#[derive(Clone)]
pub struct SpendPolicyRepository {
    policies: MongoCollection<SpendPolicy>,
    usage: MongoCollection<SpendUsage>,
}

impl SpendPolicyRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            policies: db.collection("spend_policies"),
            usage: db.collection("spend_usage"),
        }
    }

    pub async fn ensure_indices(&self) -> Result<(), AppError> {
        let policy_index = IndexModel::builder()
            .keys(doc! { "user_id": 1, "wallet_address": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        self.policies.create_index(policy_index, None).await?;

        let usage_index = IndexModel::builder()
            .keys(doc! { "wallet_address": 1, "date": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        self.usage.create_index(usage_index, None).await?;

        Ok(())
    }

    /// Upserts the one policy per (user, wallet_address) — simpler than a
    /// full CRUD list, matching this feature's "one guardrail per wallet"
    /// design rather than multiple overlapping policies to reconcile.
    pub async fn upsert(&self, policy: &SpendPolicy) -> Result<SpendPolicy, AppError> {
        let filter = doc! { "user_id": policy.user_id, "wallet_address": &policy.wallet_address };
        let update = doc! {
            "$set": mongodb::bson::to_document(policy).map_err(|e| AppError::InternalError(e.to_string()))?,
        };
        let options = FindOneAndUpdateOptions::builder()
            .upsert(true)
            .return_document(ReturnDocument::After)
            .build();

        self.policies
            .find_one_and_update(filter, update, options)
            .await?
            .ok_or_else(|| AppError::InternalError("Upsert did not return a document".into()))
    }

    pub async fn find(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
    ) -> Result<Option<SpendPolicy>, AppError> {
        Ok(self
            .policies
            .find_one(doc! { "user_id": user_id, "wallet_address": wallet_address }, None)
            .await?)
    }

    pub async fn delete(&self, user_id: ObjectId, wallet_address: &str) -> Result<(), AppError> {
        self.policies
            .delete_one(doc! { "user_id": user_id, "wallet_address": wallet_address }, None)
            .await?;
        Ok(())
    }

    /// Reads today's (UTC) usage bucket for a wallet, or a zeroed default if
    /// none exists yet — callers check this against a policy's limits before
    /// incrementing.
    pub async fn today_usage(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
    ) -> Result<SpendUsage, AppError> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let found = self
            .usage
            .find_one(doc! { "wallet_address": wallet_address, "date": &date }, None)
            .await?;

        Ok(found.unwrap_or(SpendUsage {
            id: None,
            user_id,
            wallet_address: wallet_address.to_string(),
            date,
            spent_usd: 0.0,
            tx_count: 0,
            updated_at: Utc::now(),
        }))
    }

    /// Atomically increments today's usage bucket, creating it if absent.
    pub async fn record_usage(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
        usd_value: f64,
    ) -> Result<(), AppError> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let filter = doc! { "wallet_address": wallet_address, "date": &date };
        let update = doc! {
            "$inc": { "spent_usd": usd_value, "tx_count": 1 },
            "$set": { "updated_at": mongodb::bson::DateTime::from_chrono(Utc::now()) },
            "$setOnInsert": { "user_id": user_id, "wallet_address": wallet_address, "date": &date },
        };
        let options = mongodb::options::UpdateOptions::builder().upsert(true).build();

        self.usage.update_one(filter, update, options).await?;
        Ok(())
    }
}
