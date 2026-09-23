use crate::dtos::history_dtos::{CreateHistoryEntryRequest, HistoryQuery};
use crate::services::history_service::HistoryService;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};
use std::str::FromStr;
use validator::Validate;

fn parse_workspace_id(raw: Option<&str>) -> Result<Option<ObjectId>, AppError> {
    raw.map(ObjectId::from_str)
        .transpose()
        .map_err(|_| AppError::BadRequest("Invalid workspace ID".into()))
}

pub async fn create_history_entry(
    State(service): State<HistoryService>,
    claims: Claims,
    Json(payload): Json<CreateHistoryEntryRequest>,
) -> Result<Json<Value>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let workspace_id = parse_workspace_id(payload.workspace_id.as_deref())?;

    let entry = service
        .record(
            user_id,
            workspace_id,
            payload.name,
            payload.request_type,
            payload.chain,
            payload.network,
            payload.method,
            payload.params,
            payload.tx_params,
            payload.result,
            payload.status,
            payload.duration_ms,
        )
        .await?;

    Ok(Json(serde_json::to_value(entry).unwrap()))
}

pub async fn get_history(
    State(service): State<HistoryService>,
    claims: Claims,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let workspace_id = parse_workspace_id(query.workspace_id.as_deref())?;

    let entries = service.list(user_id, workspace_id).await?;

    Ok(Json(serde_json::to_value(entries).unwrap()))
}

pub async fn clear_history(
    State(service): State<HistoryService>,
    claims: Claims,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let workspace_id = parse_workspace_id(query.workspace_id.as_deref())?;

    service.clear(user_id, workspace_id).await?;

    Ok(Json(json!({ "message": "History cleared" })))
}

pub async fn delete_history_entry(
    State(service): State<HistoryService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let entry_id =
        ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid entry ID".into()))?;

    service.delete_one(entry_id, user_id).await?;

    Ok(Json(json!({ "message": "History entry deleted" })))
}
