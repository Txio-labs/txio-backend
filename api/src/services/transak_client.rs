use reqwest::Client;

use crate::{
    dtos::bridge_dtos::{OfframpQuoteRequest, OfframpQuoteResponse},
    model::bridge_offramp::OfframpProvider,
    utils::error::AppError,
};

/// Secondary off-ramp: Transak's hosted widget. Used when Bridge.xyz isn't
/// available for the user's country/currency.
#[derive(Clone)]
pub struct TransakClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl TransakClient {
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
            .ok_or_else(|| AppError::InternalError("TRANSAK_API_KEY not configured".into()))?;

        let response = self
            .http
            .get(format!("{}/api/v1/pricing/public/quotes", self.base_url))
            .query(&[
                ("partnerApiKey", api_key.as_str()),
                ("cryptoCurrency", "USDC"),
                ("fiatCurrency", req.fiat_currency.as_str()),
                ("cryptoAmount", req.amount_usdc.as_str()),
                ("isBuyOrSell", "SELL"),
                ("network", req.source_chain.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("Transak request failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!(
                "Transak quote failed: {body}"
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::ExternalService(format!("Transak response parse failed: {e}")))?;
        let response_data = &body["response"];

        let estimated = response_data["fiatAmount"]
            .as_f64()
            .ok_or_else(|| {
                AppError::ExternalService("Transak response missing fiatAmount".into())
            })?
            .to_string();

        Ok(OfframpQuoteResponse {
            provider: OfframpProvider::Transak,
            amount_usdc: req.amount_usdc.clone(),
            estimated_fiat_amount: estimated,
            fiat_currency: req.fiat_currency.clone(),
            fee_usd_estimated: response_data["totalFee"].as_f64(),
            redirect_url: None,
            widget_config: Some(serde_json::json!({
                "apiKey": api_key,
                "cryptoCurrencyCode": "USDC",
                "fiatCurrency": req.fiat_currency,
                "network": req.source_chain,
                "productsAvailed": "SELL",
            })),
        })
    }
}
