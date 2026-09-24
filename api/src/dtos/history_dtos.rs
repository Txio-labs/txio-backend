use serde::Deserialize;
use serde_json::Value;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateHistoryEntryRequest {
    pub workspace_id: Option<String>,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    #[validate(length(min = 1, message = "Request type cannot be empty"))]
    pub request_type: String,

    pub chain: Option<String>,

    #[validate(length(min = 1, message = "Network cannot be empty"))]
    pub network: String,

    pub method: Option<String>,
    pub params: Option<Value>,
    pub wallet_family: Option<String>,
    pub wallet_address: Option<String>,
    pub tx_params: Option<Value>,
    pub result: Option<Value>,
    pub status: i32,
    pub duration_ms: i64,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub workspace_id: Option<String>,
    pub wallet_address: Option<String>,
    pub wallet_family: Option<String>,
}
