use reqwest::Client;
use serde_json::json;

use crate::{
    dtos::bridge_dtos::{QuoteRequest, QuoteResponse},
    model::{
        bridge_chain::{ChainAsset, ChainFamily},
        bridge_order::BridgeProvider,
    },
    utils::error::AppError,
};

/// Client for LI.FI's cross-chain swap/bridge aggregator. LI.FI covers a
/// broad and growing set of chains (EVM, Solana, and others) generically —
/// this client never special-cases a chain name, it just forwards whatever
/// slug the caller/registry supplies.
#[derive(Clone)]
pub struct LifiClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl LifiClient {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            http: Client::new(),
            base_url,
            api_key,
        }
    }

    pub async fn get_quote(&self, req: &QuoteRequest) -> Result<QuoteResponse, AppError> {
        let mut builder = self.http.get(format!("{}/quote", self.base_url)).query(&[
            ("fromChain", req.from_chain.as_str()),
            ("toChain", req.to_chain.as_str()),
            ("fromToken", req.from_token.as_str()),
            ("toToken", req.to_token.as_str()),
            ("fromAmount", req.amount.as_str()),
            ("fromAddress", req.from_address.as_str()),
            ("toAddress", req.to_address.as_str()),
        ]);

        if let Some(key) = &self.api_key {
            builder = builder.header("x-lifi-api-key", key);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("LI.FI request failed: {e}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound("No LI.FI route for this pair".into()));
        }
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!(
                "LI.FI quote failed: {body}"
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::ExternalService(format!("LI.FI response parse failed: {e}")))?;

        parse_lifi_quote(req, body)
    }
}

fn parse_lifi_quote(req: &QuoteRequest, body: serde_json::Value) -> Result<QuoteResponse, AppError> {
    let to_amount = body["estimate"]["toAmount"]
        .as_str()
        .ok_or_else(|| AppError::ExternalService("LI.FI response missing toAmount".into()))?
        .to_string();

    let duration = body["estimate"]["executionDuration"].as_u64();

    let fee_usd = body["estimate"]["feeCosts"].as_array().map(|fees| {
        fees.iter()
            .filter_map(|f| f["amountUsd"].as_str())
            .filter_map(|s| s.parse::<f64>().ok())
            .sum()
    });

    Ok(QuoteResponse {
        id: body["id"].as_str().unwrap_or_default().to_string(),
        provider: BridgeProvider::Lifi,
        from: ChainAsset {
            chain: req.from_chain.clone(),
            family: ChainFamily::Other("lifi".into()),
            token: req.from_token.clone(),
            symbol: body["action"]["fromToken"]["symbol"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            decimals: body["action"]["fromToken"]["decimals"].as_u64().unwrap_or(18) as u8,
        },
        to: ChainAsset {
            chain: req.to_chain.clone(),
            family: ChainFamily::Other("lifi".into()),
            token: req.to_token.clone(),
            symbol: body["action"]["toToken"]["symbol"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            decimals: body["action"]["toToken"]["decimals"].as_u64().unwrap_or(18) as u8,
        },
        from_amount: req.amount.clone(),
        to_amount_estimated: to_amount,
        fee_usd_estimated: fee_usd,
        estimated_duration_seconds: duration,
        execution_payload: json!({ "transactionRequest": body["transactionRequest"] }),
        expires_at: None,
    })
}
