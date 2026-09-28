use chrono::Utc;

use crate::{
    dtos::bridge_dtos::{ExecuteOrderRequest, QuoteRequest, QuoteResponse},
    model::bridge_order::{BridgeOrder, BridgeOrderStatus, BridgeProvider},
    repositories::bridge_order_repository::BridgeOrderRepository,
    services::{lifi_client::LifiClient, sideshift_client::SideshiftClient},
    utils::error::AppError,
};

/// Chain-agnostic swap/bridge aggregator. No chain pair is hardcoded: LI.FI
/// is tried first (broadest EVM/Solana/etc coverage); on a not-found route
/// we fall back to SideShift, which covers pairs LI.FI doesn't route (e.g.
/// Sui, Stellar) without either being special-cased in this code. The
/// caller may also pin a specific provider via `QuoteRequest.provider`.
/// txio never holds funds or keys — this only aggregates quotes and tracks
/// status; the user signs and submits every transaction themselves.
#[derive(Clone)]
pub struct BridgeService {
    lifi: LifiClient,
    sideshift: SideshiftClient,
    order_repo: BridgeOrderRepository,
}

impl BridgeService {
    pub fn new(lifi: LifiClient, sideshift: SideshiftClient, order_repo: BridgeOrderRepository) -> Self {
        Self {
            lifi,
            sideshift,
            order_repo,
        }
    }

    pub async fn get_best_quote(&self, req: &QuoteRequest) -> Result<QuoteResponse, AppError> {
        match req.provider {
            Some(BridgeProvider::Lifi) => self.lifi.get_quote(req).await,
            Some(BridgeProvider::Sideshift) => self.sideshift.get_quote(req).await,
            None => match self.lifi.get_quote(req).await {
                Ok(quote) => Ok(quote),
                Err(AppError::NotFound(_)) | Err(AppError::ExternalService(_)) => {
                    self.sideshift.get_quote(req).await
                }
                Err(other) => Err(other),
            },
        }
    }

    /// Records that the user is executing a previously-fetched quote. For
    /// LI.FI this is purely informational (the client signs/sends the
    /// transaction itself). For SideShift this also creates the
    /// deposit-address order.
    pub async fn execute_order(
        &self,
        user_id: &str,
        req: &ExecuteOrderRequest,
    ) -> Result<(BridgeOrder, serde_json::Value), AppError> {
        let (provider_order_id, extra) = match req.provider {
            BridgeProvider::Sideshift => {
                let order = self
                    .sideshift
                    .create_order(&req.quote_id, &req.to_address)
                    .await?;
                let id = order["id"]
                    .as_str()
                    .ok_or_else(|| {
                        AppError::ExternalService("SideShift order missing id".into())
                    })?
                    .to_string();
                (id, order)
            }
            BridgeProvider::Lifi => (req.quote_id.clone(), serde_json::json!({})),
        };

        let now = Utc::now();
        let order = BridgeOrder {
            id: None,
            user_id: user_id.to_string(),
            quote_id: req.quote_id.clone(),
            provider: req.provider,
            provider_order_id,
            from_chain: req.from_chain.clone(),
            from_token: req.from_token.clone(),
            to_chain: req.to_chain.clone(),
            to_token: req.to_token.clone(),
            from_address: req.from_address.clone(),
            to_address: req.to_address.clone(),
            status: BridgeOrderStatus::AwaitingDeposit,
            tx_hash: None,
            created_at: now,
            updated_at: now,
        };

        self.order_repo.insert(&order).await?;

        Ok((order, extra))
    }

    pub async fn get_order_status(
        &self,
        user_id: &str,
        provider_order_id: &str,
    ) -> Result<BridgeOrder, AppError> {
        let order = self
            .order_repo
            .find_by_provider_order_id(provider_order_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Order not found".into()))?;

        if order.user_id != user_id {
            return Err(AppError::Forbidden(
                "You do not have access to this order".into(),
            ));
        }

        if order.provider == BridgeProvider::Sideshift {
            if let Ok(remote) = self.sideshift.get_order_status(provider_order_id).await {
                if let Some(status_str) = remote["status"].as_str() {
                    let mapped = map_sideshift_status(status_str);
                    let tx_hash = remote["settleHash"].as_str().map(str::to_string);
                    self.order_repo
                        .update_status(provider_order_id, mapped, tx_hash.clone())
                        .await?;

                    let mut updated = order;
                    updated.status = mapped;
                    updated.tx_hash = tx_hash;
                    return Ok(updated);
                }
            }
        }

        Ok(order)
    }
}

fn map_sideshift_status(status: &str) -> BridgeOrderStatus {
    match status {
        "waiting" => BridgeOrderStatus::AwaitingDeposit,
        "pending" | "processing" | "confirming" | "settling" => BridgeOrderStatus::Processing,
        "settled" => BridgeOrderStatus::Completed,
        "refund" | "refunding" | "refunded" => BridgeOrderStatus::Refunded,
        "expired" => BridgeOrderStatus::Expired,
        _ => BridgeOrderStatus::Failed,
    }
}
