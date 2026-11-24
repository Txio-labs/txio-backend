use crate::services::stellar_indexer::{StellarIndexer, MAX_HISTORY_LIMIT};
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub struct ContractEventsQuery {
    pub network: Option<String>,
    pub limit: Option<i64>,
}

/// A Soroban contract address: `C` followed by 55 base32 characters.
fn is_contract_address(raw: &str) -> bool {
    raw.len() == 56
        && raw.starts_with('C')
        && raw.chars().all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
}

/// Events for a `C...` contract. First lookup registers the contract; the
/// `backfilled` flag tells the UI whether the stored history is complete yet.
pub async fn get_contract_events(
    State(indexer): State<StellarIndexer>,
    _claims: Claims,
    Path(contract_id): Path<String>,
    Query(query): Query<ContractEventsQuery>,
) -> Result<Json<Value>, AppError> {
    if !is_contract_address(&contract_id) {
        return Err(AppError::BadRequest(
            "Expected a Soroban contract address (C... with 56 characters).".into(),
        ));
    }
    let network = query.network.unwrap_or_else(|| "mainnet".to_string());
    if !indexer.supports(&network) {
        return Err(AppError::BadRequest(format!(
            "Contract history is not indexed on {network}."
        )));
    }
    let limit = query.limit.unwrap_or(20).clamp(1, MAX_HISTORY_LIMIT);
    let (events, backfilled) = indexer.history(&network, &contract_id, limit).await?;

    Ok(Json(json!({
        "contractId": contract_id,
        "network": network,
        "backfilled": backfilled,
        "events": events,
    })))
}

/// A `G...` or `C...` holder address (56 characters, strkey base32).
fn is_holder_address(raw: &str) -> bool {
    raw.len() == 56
        && (raw.starts_with('G') || raw.starts_with('C'))
        && raw.chars().all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
}

/// Net token balances for an address, from indexed SEP-41 transfers.
pub async fn get_token_balances(
    State(indexer): State<StellarIndexer>,
    _claims: Claims,
    Path(address): Path<String>,
    Query(query): Query<ContractEventsQuery>,
) -> Result<Json<Value>, AppError> {
    if !is_holder_address(&address) {
        return Err(AppError::BadRequest(
            "Expected a Stellar account (G...) or contract (C...) address.".into(),
        ));
    }
    let network = query.network.unwrap_or_else(|| "mainnet".to_string());
    if !indexer.supports(&network) {
        return Err(AppError::BadRequest(format!("Token balances are not indexed on {network}.")));
    }
    let balances = indexer.token_balances(&network, &address).await?;
    Ok(Json(json!({
        "address": address,
        "network": network,
        "tokens": balances,
    })))
}
