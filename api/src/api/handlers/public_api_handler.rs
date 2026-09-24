use crate::dtos::public_api_dtos::{ExecuteTransactionRequest, PublicHistoryQuery, SimulateTransactionRequest};
use crate::services::public_api_service::PublicApiService;
use crate::utils::api_key_auth::ApiKeyAuth;
use crate::utils::error::AppError;
use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::{json, Value};

pub async fn get_history(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Query(query): Query<PublicHistoryQuery>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("history:read")?;
    let entries = service
        .history(auth.user_id, query.wallet_address, query.wallet_family, query.chain)
        .await?;
    Ok(Json(serde_json::to_value(entries).unwrap()))
}

pub async fn simulate_transaction(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Json(payload): Json<SimulateTransactionRequest>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("transactions:simulate")?;
    Ok(Json(serde_json::to_value(service.simulate(&payload.chain)).unwrap()))
}

pub async fn execute_transaction(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Json(payload): Json<ExecuteTransactionRequest>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("transactions:execute")?;
    let hash = service.execute(auth.user_id, payload).await?;
    Ok(Json(json!({ "hash": hash })))
}
