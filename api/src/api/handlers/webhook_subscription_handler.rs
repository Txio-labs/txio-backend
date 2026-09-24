use crate::dtos::webhook_subscription_dtos::{CreateWebhookRequest, CreateWebhookResponse};
use crate::services::webhook_service::WebhookService;
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

pub async fn create_webhook(
    State(service): State<WebhookService>,
    claims: Claims,
    Json(payload): Json<CreateWebhookRequest>,
) -> Result<Json<CreateWebhookResponse>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    let (sub, secret) = service.create(user_id(&claims)?, payload).await?;
    Ok(Json(CreateWebhookResponse {
        id: sub.id.map(|i| i.to_hex()).unwrap_or_default(),
        url: sub.url,
        events: sub.events,
        secret,
    }))
}

pub async fn list_webhooks(
    State(service): State<WebhookService>,
    claims: Claims,
) -> Result<Json<Value>, AppError> {
    let subs = service.list(user_id(&claims)?).await?;
    Ok(Json(serde_json::to_value(subs).unwrap()))
}

pub async fn delete_webhook(
    State(service): State<WebhookService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let sub_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid webhook id".into()))?;
    service.delete(sub_id, user_id(&claims)?).await?;
    Ok(Json(json!({ "message": "Webhook deleted" })))
}
