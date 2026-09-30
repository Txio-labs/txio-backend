use serde::Deserialize;
use serde_json::Value;

/// A history entry as the public API returns it: the documented fields only,
/// with the id as a plain string and without internal ids (user, workspace).
#[derive(Debug, serde::Serialize)]
pub struct PublicHistoryEntry {
    pub id: String,
    pub name: String,
    pub request_type: String,
    pub chain: Option<String>,
    pub network: String,
    pub method: Option<String>,
    pub wallet_family: Option<String>,
    pub wallet_address: Option<String>,
    pub tx_params: Option<Value>,
    pub result: Option<Value>,
    pub status: i32,
    pub duration_ms: i64,
    pub executed_at: String,
}

impl From<crate::model::history::HistoryEntry> for PublicHistoryEntry {
    fn from(e: crate::model::history::HistoryEntry) -> Self {
        Self {
            id: e.id.map(|i| i.to_hex()).unwrap_or_default(),
            name: e.name,
            request_type: e.request_type,
            chain: e.chain,
            network: e.network,
            method: e.method,
            wallet_family: e.wallet_family,
            wallet_address: e.wallet_address,
            tx_params: e.tx_params,
            result: e.result,
            status: e.status,
            duration_ms: e.duration_ms,
            executed_at: e.executed_at.to_rfc3339(),
        }
    }
}

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

#[derive(Debug, Deserialize)]
pub struct ExecuteTransactionRequest {
    pub session_key_id: String,
    pub chain: String,
    pub tx_params: Value,
}
