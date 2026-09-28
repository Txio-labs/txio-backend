use axum::{
    extract::{Path, State},
    Json,
};
use validator::Validate;

use crate::{
    dtos::bridge_dtos::{
        ExecuteOrderRequest, OfframpQuoteRequest, OfframpQuoteResponse, QuoteRequest,
        QuoteResponse,
    },
    model::bridge_order::BridgeOrder,
    services::{bridge_service::BridgeService, offramp_service::OfframpService},
    utils::auth_jwt::Claims,
    utils::error::AppError,
};

pub async fn get_quote(
    _claims: Claims,
    State(service): State<BridgeService>,
    Json(payload): Json<QuoteRequest>,
) -> Result<Json<QuoteResponse>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let quote = service.get_best_quote(&payload).await?;
    Ok(Json(quote))
}

pub async fn execute_order(
    claims: Claims,
    State(service): State<BridgeService>,
    Json(payload): Json<ExecuteOrderRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let (order, provider_response) = service.execute_order(&claims.sub, &payload).await?;

    Ok(Json(serde_json::json!({
        "order": order,
        "provider_response": provider_response,
    })))
}

pub async fn get_order_status(
    claims: Claims,
    State(service): State<BridgeService>,
    Path(provider_order_id): Path<String>,
) -> Result<Json<BridgeOrder>, AppError> {
    let order = service
        .get_order_status(&claims.sub, &provider_order_id)
        .await?;
    Ok(Json(order))
}

pub async fn get_offramp_quote(
    _claims: Claims,
    State(service): State<OfframpService>,
    Json(payload): Json<OfframpQuoteRequest>,
) -> Result<Json<OfframpQuoteResponse>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let quote = service.get_best_quote(&payload).await?;
    Ok(Json(quote))
}
