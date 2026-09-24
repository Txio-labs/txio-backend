use crate::dtos::spend_policy_dtos::{CheckSpendRequest, SpendCheckResponse, SpendPolicyQuery, UpsertSpendPolicyRequest};
use crate::services::spend_policy_service::SpendPolicyService;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{
    extract::{Query, State},
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde_json::{json, Value};
use std::str::FromStr;
use validator::Validate;

fn user_id(claims: &Claims) -> Result<ObjectId, AppError> {
    ObjectId::from_str(&claims.sub).map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))
}

pub async fn upsert_policy(
    State(service): State<SpendPolicyService>,
    claims: Claims,
    Json(payload): Json<UpsertSpendPolicyRequest>,
) -> Result<Json<Value>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    let policy = service.upsert(user_id(&claims)?, payload).await?;
    Ok(Json(serde_json::to_value(policy).unwrap()))
}

pub async fn get_policy(
    State(service): State<SpendPolicyService>,
    claims: Claims,
    Query(query): Query<SpendPolicyQuery>,
) -> Result<Json<Value>, AppError> {
    let policy = service.get(user_id(&claims)?, &query.wallet_address).await?;
    Ok(Json(serde_json::to_value(policy).unwrap()))
}

pub async fn delete_policy(
    State(service): State<SpendPolicyService>,
    claims: Claims,
    Query(query): Query<SpendPolicyQuery>,
) -> Result<Json<Value>, AppError> {
    service.delete(user_id(&claims)?, &query.wallet_address).await?;
    Ok(Json(json!({ "message": "Spend policy removed" })))
}

/// Lets the frontend pre-check a simulated transaction's USD value against
/// the wallet's policy before offering to sign — the same `check` the
/// scheduler worker's unattended path enforces server-side regardless.
pub async fn check_spend(
    State(service): State<SpendPolicyService>,
    claims: Claims,
    Json(payload): Json<CheckSpendRequest>,
) -> Result<Json<SpendCheckResponse>, AppError> {
    match service.check(user_id(&claims)?, &payload.wallet_address, payload.usd_value).await {
        Ok(()) => Ok(Json(SpendCheckResponse { allowed: true, reason: None })),
        Err(AppError::Forbidden(reason)) => Ok(Json(SpendCheckResponse { allowed: false, reason: Some(reason) })),
        Err(other) => Err(other),
    }
}
