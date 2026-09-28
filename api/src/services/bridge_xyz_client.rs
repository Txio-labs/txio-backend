use reqwest::Client;
use serde_json::json;

use crate::{
    dtos::bridge_dtos::{OfframpQuoteRequest, OfframpQuoteResponse},
    model::bridge_offramp::OfframpProvider,
    utils::error::AppError,
};

/// Client for Bridge.xyz's USDC -> fiat off-ramp. txio-bridge never
/// collects KYC or bank details itself — it forwards to Bridge.xyz's
/// hosted flow and only tracks status.
#[derive(Clone)]
pub struct BridgeXyzClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl BridgeXyzClient {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            http: Client::new(),
            base_url,
            api_key,
        }
    }

    pub async fn get_quote(
        &self,
        req: &OfframpQuoteRequest,
    ) -> Result<OfframpQuoteResponse, AppError> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| AppError::InternalError("BRIDGE_XYZ_API_KEY not configured".into()))?;

        let response = self
            .http
            .post(format!("{}/v0/exchange_rates", self.base_url))
            .header("Api-Key", api_key)
            .json(&json!({
                "from_currency": "usdc",
                "to_currency": req.fiat_currency.to_lowercase(),
                "amount": req.amount_usdc,
            }))
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("Bridge.xyz request failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!(
                "Bridge.xyz quote failed: {body}"
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            AppError::ExternalService(format!("Bridge.xyz response parse failed: {e}"))
        })?;

        let estimated = body["to_amount"]
            .as_str()
            .ok_or_else(|| {
                AppError::ExternalService("Bridge.xyz response missing to_amount".into())
            })?
            .to_string();

        Ok(OfframpQuoteResponse {
            provider: OfframpProvider::BridgeXyz,
            amount_usdc: req.amount_usdc.clone(),
            estimated_fiat_amount: estimated,
            fiat_currency: req.fiat_currency.clone(),
            fee_usd_estimated: body["fee"].as_f64(),
            redirect_url: body["hosted_url"].as_str().map(str::to_string),
            widget_config: None,
        })
    }
}
