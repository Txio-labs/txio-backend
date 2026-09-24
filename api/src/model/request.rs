use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct SavedRequest {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub collection_id: ObjectId,
    pub user_id: ObjectId,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    /// Empty for a TRANSACTION request — its target lives in `tx_params`
    /// instead. Required (see CreateSavedRequestRequest) for RPC requests.
    pub method: String,

    pub params: serde_json::Value,

    /// "RPC" or "TRANSACTION" — mirrors the frontend's RequestType enum.
    /// Defaulted for requests saved before this field existed.
    #[serde(default = "default_request_type")]
    pub request_type: String,

    /// Which chain this request targets (frontend ChainId: "sui", "evm",
    /// "solana", "stellar"). Absent on legacy RPC requests, which default to
    /// Sui in the frontend when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,

    /// Chain-native transaction params (Sui moveParams, EVM evmTxParams,
    /// Solana solanaTxParams, Stellar stellarTxParams) for a TRANSACTION
    /// request — opaque to the backend, replayed as-is by the frontend.
    /// Mirrors HistoryEntry.tx_params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_params: Option<serde_json::Value>,

    pub network: Option<String>,
    pub rpc_url: Option<String>,

    pub last_response: Option<serde_json::Value>,
    #[serde(
        default,
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub last_executed_at: Option<DateTime<Utc>>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

fn default_request_type() -> String {
    "RPC".to_string()
}

impl SavedRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        collection_id: ObjectId,
        user_id: ObjectId,
        name: String,
        method: String,
        params: serde_json::Value,
        request_type: String,
        chain: Option<String>,
        tx_params: Option<serde_json::Value>,
        network: Option<String>,
        rpc_url: Option<String>,
    ) -> Self {
        Self {
            id: None,
            collection_id,
            user_id,
            name,
            method,
            params,
            request_type,
            chain,
            tx_params,
            network,
            rpc_url,
            last_response: None,
            last_executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}
