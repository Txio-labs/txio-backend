use crate::dtos::public_api_dtos::{
    AddressLabelRequest, AddressLabelResponse, AddressOwnerQuery, ExecuteTransactionRequest, PublicHistoryEntry,
    PublicHistoryQuery, SimulateTransactionRequest,
};
use crate::model::address_label::{normalize_address, validate_label, LOOKUP_CHAINS};
use crate::services::public_api_service::PublicApiService;
use crate::utils::api_key_auth::ApiKeyAuth;
use crate::utils::error::AppError;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
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
    let entries: Vec<PublicHistoryEntry> = entries.into_iter().map(Into::into).collect();
    Ok(Json(serde_json::to_value(entries).map_err(|_| AppError::InternalError("Serialization failed".into()))?))
}

fn check_chain(raw: &str) -> Result<&'static str, AppError> {
    LOOKUP_CHAINS
        .iter()
        .copied()
        .find(|c| *c == raw.trim())
        .ok_or_else(|| AppError::BadRequest(format!("chain must be one of: {}", LOOKUP_CHAINS.join(", "))))
}

fn check_address(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        return Err(AppError::BadRequest("address must be 1 to 128 characters".into()));
    }
    Ok(normalize_address(trimmed))
}

pub async fn get_address_owners(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Query(query): Query<AddressOwnerQuery>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("addresses:read")?;
    check_address(&query.address)?;
    let chain = query.chain.as_deref().map(check_chain).transpose()?;
    let response = service.address_owners(auth.user_id, query.address.trim(), chain).await?;
    Ok(Json(serde_json::to_value(response).map_err(|_| AppError::InternalError("Serialization failed".into()))?))
}

pub async fn put_address_label(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Json(payload): Json<AddressLabelRequest>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("addresses:write")?;
    let chain = check_chain(&payload.chain)?;
    let address = check_address(&payload.address)?;
    let label = validate_label(&payload.label).map_err(AppError::BadRequest)?;
    service.set_label(auth.user_id, chain, &address, &label).await?;
    let response = AddressLabelResponse { address, chain: chain.to_string(), label };
    Ok(Json(serde_json::to_value(response).map_err(|_| AppError::InternalError("Serialization failed".into()))?))
}

pub async fn simulate_transaction(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    Json(payload): Json<SimulateTransactionRequest>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("transactions:simulate")?;
    service.simulate(&payload.chain)?;
    unreachable_simulate()
}

pub async fn execute_transaction(
    State(service): State<PublicApiService>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    Json(payload): Json<ExecuteTransactionRequest>,
) -> Result<Json<Value>, AppError> {
    auth.require_scope("transactions:execute")?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if idempotency_key.is_some_and(|k| k.len() > 128) {
        return Err(AppError::BadRequest("Idempotency-Key must be at most 128 characters".into()));
    }
    let hash = service.execute_idempotent(auth.user_id, idempotency_key, payload).await?;
    Ok(Json(json!({ "hash": hash })))
}

/// `simulate` currently always returns an error before reaching this; kept as a
/// function so the success shape is added in one place when it is implemented.
fn unreachable_simulate() -> Result<Json<Value>, AppError> {
    Err(AppError::InternalError("simulate returned without a result".into()))
}

pub async fn openapi_document() -> Json<Value> {
    Json(crate::api::openapi::document())
}
