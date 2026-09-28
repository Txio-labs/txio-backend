use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfframpProvider {
    BridgeXyz,
    Transak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfframpStatus {
    Pending,
    KycRequired,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfframpOrder {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<mongodb::bson::oid::ObjectId>,
    pub user_id: String,
    pub provider: OfframpProvider,
    pub provider_order_id: String,
    pub amount_usdc: String,
    pub fiat_currency: String,
    pub status: OfframpStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
