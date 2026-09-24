use crate::dtos::session_key_dtos::CreateSessionKeyRequest;
use crate::services::session_key_service::SessionKeyService;
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

pub async fn create_session_key(
    State(service): State<SessionKeyService>,
    claims: Claims,
    Json(payload): Json<CreateSessionKeyRequest>,
) -> Result<Json<Value>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    let key = service.create(user_id(&claims)?, payload).await?;
    Ok(Json(serde_json::to_value(key).unwrap()))
}

pub async fn list_session_keys(
    State(service): State<SessionKeyService>,
    claims: Claims,
) -> Result<Json<Value>, AppError> {
    let keys = service.list(user_id(&claims)?).await?;
    Ok(Json(serde_json::to_value(keys).unwrap()))
}

pub async fn revoke_session_key(
    State(service): State<SessionKeyService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let key_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid session key id".into()))?;
    service.revoke(key_id, user_id(&claims)?).await?;
    Ok(Json(json!({ "message": "Session key revoked" })))
}
