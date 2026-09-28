use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeProvider {
    Lifi,
    Sideshift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeOrderStatus {
    Pending,
    AwaitingDeposit,
    Processing,
    Completed,
    Failed,
    Refunded,
    Expired,
}

/// A persisted record of a swap/bridge the user executed client-side.
/// txio-bridge never holds funds or keys — this row exists purely to track
/// status for the UI/history, mirroring what the provider reports back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeOrder {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<mongodb::bson::oid::ObjectId>,
    pub user_id: String,
    pub quote_id: String,
    pub provider: BridgeProvider,
    pub provider_order_id: String,
    pub from_chain: String,
    pub from_token: String,
    pub to_chain: String,
    pub to_token: String,
    pub from_address: String,
    pub to_address: String,
    pub status: BridgeOrderStatus,
    pub tx_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
