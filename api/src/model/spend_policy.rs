use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use validator::Validate;

/// A per-wallet spend guardrail: caps and rate limits enforced before a
/// transaction (interactive or automated) is allowed to execute. One policy
/// per (user, wallet_address) — later automation features (session keys,
/// scheduled execution) reuse this same check, since unattended execution is
/// exactly the case these limits exist to constrain.
#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct SpendPolicy {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,

    pub wallet_family: String,

    #[validate(length(min = 1, message = "Wallet address cannot be empty"))]
    pub wallet_address: String,

    /// `None` applies the policy across every chain this wallet acts on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_limit_usd: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_tx_limit_usd: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tx_per_hour: Option<u32>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl SpendPolicy {
    pub fn new(
        user_id: ObjectId,
        wallet_family: String,
        wallet_address: String,
        chain: Option<String>,
        daily_limit_usd: Option<f64>,
        per_tx_limit_usd: Option<f64>,
        max_tx_per_hour: Option<u32>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            user_id,
            wallet_family,
            wallet_address,
            chain,
            daily_limit_usd,
            per_tx_limit_usd,
            max_tx_per_hour,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Rolling usage tracked against a policy — one document per (wallet, UTC
/// day) bucket, incremented on every execution so enforcement never has to
/// recompute totals from full history.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SpendUsage {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,
    pub wallet_address: String,

    /// UTC date this bucket covers, formatted `YYYY-MM-DD`.
    pub date: String,

    pub spent_usd: f64,
    pub tx_count: u32,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}
