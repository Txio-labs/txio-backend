use crate::dtos::api_key_dtos::{CreateApiKeyRequest, CreateApiKeyResponse};
use crate::services::api_key_service::ApiKeyService;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{
    extract::{Path, State},
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde_json::{json, Value};
use std::str::FromStr;
use validator::Validate;

fn user_id(claims: &Claims) -> Result<ObjectId, AppError> {
    ObjectId::from_str(&claims.sub).map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))
}

pub async fn create_api_key(
    State(service): State<ApiKeyService>,
    claims: Claims,
    Json(payload): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    let (key, raw_key) = service.create(user_id(&claims)?, payload.label, payload.scopes).await?;
    Ok(Json(CreateApiKeyResponse {
        id: key.id.map(|i| i.to_hex()).unwrap_or_default(),
        label: key.label,
        key_prefix: key.key_prefix,
        scopes: key.scopes,
        key: raw_key,
    }))
}

pub async fn list_api_keys(
    State(service): State<ApiKeyService>,
    claims: Claims,
) -> Result<Json<Value>, AppError> {
    let keys = service.list(user_id(&claims)?).await?;
    Ok(Json(serde_json::to_value(keys).unwrap()))
}

pub async fn revoke_api_key(
    State(service): State<ApiKeyService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let key_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid API key id".into()))?;
    service.revoke(key_id, user_id(&claims)?).await?;
    Ok(Json(json!({ "message": "API key revoked" })))
}
