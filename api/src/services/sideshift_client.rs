use chrono::Utc;
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

/// Client for SideShift.ai — used as the fallback aggregator for any pair
/// LI.FI doesn't route (notably Sui and Stellar today, but this client
/// doesn't hardcode that; it just reports whatever SideShift accepts).
#[derive(Clone)]
pub struct SideshiftClient {
    http: Client,
    base_url: String,
    secret: Option<String>,
    affiliate_id: Option<String>,
}

impl SideshiftClient {
    pub fn new(base_url: String, secret: Option<String>, affiliate_id: Option<String>) -> Self {
        Self {
            http: Client::new(),
            base_url,
            secret,
            affiliate_id,
        }
    }

    pub async fn get_quote(&self, req: &QuoteRequest) -> Result<QuoteResponse, AppError> {
        let mut body = json!({
            "depositCoin": req.from_token,
            "depositNetwork": req.from_chain,
            "settleCoin": req.to_token,
            "settleNetwork": req.to_chain,
            "depositAmount": req.amount,
        });

        tracing::debug!(
            deposit_coin = %req.from_token,
            deposit_network = %req.from_chain,
            settle_coin = %req.to_token,
            settle_network = %req.to_chain,
            deposit_amount = %req.amount,
            "SideShift quote request"
        );
        // SideShift rejects `affiliateId: null` outright (BAD_USER_INPUT) —
        // the field must be omitted entirely when unset, not sent as null.
        if let Some(affiliate_id) = &self.affiliate_id {
            body["affiliateId"] = json!(affiliate_id);
        }

        let mut builder = self.http.post(format!("{}/quotes", self.base_url)).json(&body);

        if let Some(secret) = &self.secret {
            builder = builder.header("x-sideshift-secret", secret);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("SideShift request failed: {e}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound(
                "No SideShift route for this pair".into(),
            ));
        }
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            // SideShift geo-blocks some jurisdictions (including the US) at
            // the account/IP level, independent of the request payload —
            // this is safe to tell the caller directly (unlike other
            // ExternalService failures, which stay server-side-only) since
            // it explains a real, permanent condition rather than leaking
            // upstream internals.
            if body.contains("ACCESS_DENIED") {
                return Err(AppError::BadRequest(
                    "SideShift is not available from this server's hosting region right now. Try again later or use a different provider.".into(),
                ));
            }
            return Err(AppError::ExternalService(format!(
                "SideShift quote failed: {body}"
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            AppError::ExternalService(format!("SideShift response parse failed: {e}"))
        })?;

        let settle_amount = body["settleAmount"]
            .as_str()
            .ok_or_else(|| {
                AppError::ExternalService("SideShift response missing settleAmount".into())
            })?
            .to_string();

        Ok(QuoteResponse {
            id: body["id"].as_str().unwrap_or_default().to_string(),
            provider: BridgeProvider::Sideshift,
            from: ChainAsset {
                chain: req.from_chain.clone(),
                family: ChainFamily::Other("sideshift".into()),
                token: req.from_token.clone(),
                symbol: req.from_token.to_uppercase(),
                decimals: 0,
            },
            to: ChainAsset {
                chain: req.to_chain.clone(),
                family: ChainFamily::Other("sideshift".into()),
                token: req.to_token.clone(),
                symbol: req.to_token.to_uppercase(),
                decimals: 0,
            },
            from_amount: req.amount.clone(),
            to_amount_estimated: settle_amount,
            fee_usd_estimated: None,
            estimated_duration_seconds: None,
            execution_payload: json!({ "sideshiftQuoteId": body["id"] }),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(15)),
        })
    }

    /// Creates the actual deposit-address order once the user accepts a quote.
    ///
    /// Unlike `/quotes`, SideShift's `/shifts/fixed` endpoint requires a
    /// *registered* affiliateId (sign up at sideshift.ai) — it 400s with
    /// "Unknown affiliateId" for anything else, and with "expected string,
    /// received undefined" if the field is omitted. So this fails fast with
    /// a clear message instead of forwarding a request SideShift will
    /// always reject.
    pub async fn create_order(
        &self,
        quote_id: &str,
        settle_address: &str,
    ) -> Result<serde_json::Value, AppError> {
        let affiliate_id = self.affiliate_id.as_ref().ok_or_else(|| {
            AppError::InternalError(
                "SIDESHIFT_AFFILIATE_ID not configured — register an affiliate account at sideshift.ai to enable order execution".into(),
            )
        })?;

        let body = json!({
            "quoteId": quote_id,
            "settleAddress": settle_address,
            "affiliateId": affiliate_id,
        });

        let mut builder = self
            .http
            .post(format!("{}/shifts/fixed", self.base_url))
            .json(&body);

        if let Some(secret) = &self.secret {
            builder = builder.header("x-sideshift-secret", secret);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("SideShift request failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!(
                "SideShift order creation failed: {body}"
            )));
        }

        response
            .json()
            .await
            .map_err(|e| AppError::ExternalService(format!("SideShift response parse failed: {e}")))
    }

    pub async fn get_order_status(&self, shift_id: &str) -> Result<serde_json::Value, AppError> {
        let response = self
            .http
            .get(format!("{}/shifts/{shift_id}", self.base_url))
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("SideShift request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(AppError::NotFound("SideShift order not found".into()));
        }

        response
            .json()
            .await
            .map_err(|e| AppError::ExternalService(format!("SideShift response parse failed: {e}")))
    }
}
