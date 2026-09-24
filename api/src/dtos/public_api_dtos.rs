use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct PublicHistoryQuery {
    pub wallet_address: Option<String>,
    pub wallet_family: Option<String>,
    pub chain: Option<String>,
}

/// Chain-native transaction params, opaque here exactly as History already
/// treats them (see model::history::HistoryEntry.tx_params) — the public
/// API doesn't re-validate chain-specific shape, the same adapter the
/// internal UI uses does that when this is actually executed.
#[derive(Debug, Deserialize)]
pub struct SimulateTransactionRequest {
    pub chain: String,
    pub tx_params: Value,
}

#[derive(Debug, Serialize)]
pub struct SimulateTransactionResponse {
    pub chain: String,
    /// Placeholder pass-through — see api_transaction_handler.rs doc
    /// comment for why real simulation isn't wired up yet.
    pub simulated: bool,
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteTransactionRequest {
    pub session_key_id: String,
    pub chain: String,
    pub tx_params: Value,
}
