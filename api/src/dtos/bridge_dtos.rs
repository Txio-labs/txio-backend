use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::model::{bridge_chain::ChainAsset, bridge_order::BridgeProvider};

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct QuoteRequest {
    #[validate(length(min = 1))]
    pub from_chain: String,
    #[validate(length(min = 1))]
    pub from_token: String,
    #[validate(length(min = 1))]
    pub to_chain: String,
    #[validate(length(min = 1))]
    pub to_token: String,
    /// Amount in the smallest unit of `from_token` (string to avoid precision loss).
    #[validate(length(min = 1))]
    pub amount: String,
    #[validate(length(min = 1))]
    pub from_address: String,
    #[validate(length(min = 1))]
    pub to_address: String,
    /// Restrict to a specific provider; omit to let the aggregator pick the best route.
    pub provider: Option<BridgeProvider>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuoteResponse {
    pub id: String,
    pub provider: BridgeProvider,
    pub from: ChainAsset,
    pub to: ChainAsset,
    pub from_amount: String,
    pub to_amount_estimated: String,
    pub fee_usd_estimated: Option<f64>,
    pub estimated_duration_seconds: Option<u64>,
    /// Opaque provider payload the caller submits back at execution time
    /// (e.g. LI.FI's `transactionRequest`, or a SideShift deposit address).
    pub execution_payload: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct ExecuteOrderRequest {
    #[validate(length(min = 1))]
    pub quote_id: String,
    pub provider: BridgeProvider,
    #[validate(length(min = 1))]
    pub from_chain: String,
    #[validate(length(min = 1))]
    pub from_token: String,
    #[validate(length(min = 1))]
    pub to_chain: String,
    #[validate(length(min = 1))]
    pub to_token: String,
    #[validate(length(min = 1))]
    pub from_address: String,
    #[validate(length(min = 1))]
    pub to_address: String,
}

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct OfframpQuoteRequest {
    #[validate(length(min = 1))]
    pub source_chain: String,
    #[validate(length(min = 1))]
    pub amount_usdc: String,
    #[validate(length(min = 3, max = 3))]
    pub fiat_currency: String,
    #[validate(length(min = 2, max = 2))]
    pub country: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OfframpQuoteResponse {
    pub provider: crate::model::bridge_offramp::OfframpProvider,
    pub amount_usdc: String,
    pub estimated_fiat_amount: String,
    pub fiat_currency: String,
    pub fee_usd_estimated: Option<f64>,
    pub redirect_url: Option<String>,
    pub widget_config: Option<serde_json::Value>,
}
