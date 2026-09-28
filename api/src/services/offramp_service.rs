use crate::{
    dtos::bridge_dtos::{OfframpQuoteRequest, OfframpQuoteResponse},
    services::{bridge_xyz_client::BridgeXyzClient, transak_client::TransakClient},
    utils::error::AppError,
};

/// Bridge.xyz is preferred; Transak is the secondary path when Bridge.xyz
/// isn't configured or fails for the requested country/currency.
#[derive(Clone)]
pub struct OfframpService {
    bridge_xyz: BridgeXyzClient,
    transak: TransakClient,
}

impl OfframpService {
    pub fn new(bridge_xyz: BridgeXyzClient, transak: TransakClient) -> Self {
        Self {
            bridge_xyz,
            transak,
        }
    }

    pub async fn get_best_quote(
        &self,
        req: &OfframpQuoteRequest,
    ) -> Result<OfframpQuoteResponse, AppError> {
        match self.bridge_xyz.get_quote(req).await {
            Ok(quote) => Ok(quote),
            Err(_) => self.transak.get_quote(req).await,
        }
    }
}
