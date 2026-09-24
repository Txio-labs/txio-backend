use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct UpsertSpendPolicyRequest {
    pub wallet_family: String,

    #[validate(length(min = 1, message = "Wallet address cannot be empty"))]
    pub wallet_address: String,

    pub chain: Option<String>,

    #[validate(range(min = 0.0, message = "Daily limit must be non-negative"))]
    pub daily_limit_usd: Option<f64>,

    #[validate(range(min = 0.0, message = "Per-transaction limit must be non-negative"))]
    pub per_tx_limit_usd: Option<f64>,

    pub max_tx_per_hour: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct SpendPolicyQuery {
    pub wallet_address: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckSpendRequest {
    pub wallet_address: String,
    pub usd_value: f64,
}

#[derive(Debug, Serialize)]
pub struct SpendCheckResponse {
    pub allowed: bool,
    pub reason: Option<String>,
}
