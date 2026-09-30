use crate::dtos::webhook_subscription_dtos::{
    CreateWebhookRequest, CreateWebhookResponse, RotateSecretResponse, WebhookDeliveryResponse,
    WebhookSubscriptionResponse,
};
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
    let body: Vec<WebhookSubscriptionResponse> = subs
        .into_iter()
        .map(|s| WebhookSubscriptionResponse {
            id: s.id.map(|i| i.to_hex()).unwrap_or_default(),
            url: s.url,
            events: s.events,
            is_active: s.is_active,
            can_sign: s.secret_enc.is_some(),
            last_delivered_at: s.last_delivered_at.map(|t| t.to_rfc3339()),
            last_delivery_error: s.last_delivery_error,
            created_at: s.created_at.to_rfc3339(),
        })
        .collect();
    Ok(Json(serde_json::to_value(body).map_err(|_| AppError::InternalError("Serialization failed".into()))?))
}

fn parse_id(raw: &str, what: &str) -> Result<ObjectId, AppError> {
    ObjectId::from_str(raw).map_err(|_| AppError::BadRequest(format!("Invalid {what}")))
}

pub async fn rotate_secret(
    State(service): State<WebhookService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<RotateSecretResponse>, AppError> {
    let secret = service.rotate_secret(parse_id(&id, "webhook id")?, user_id(&claims)?).await?;
    Ok(Json(RotateSecretResponse { secret }))
}

pub async fn list_deliveries(
    State(service): State<WebhookService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Vec<WebhookDeliveryResponse>>, AppError> {
    let deliveries = service.recent_deliveries(parse_id(&id, "webhook id")?, user_id(&claims)?).await?;
    Ok(Json(
        deliveries
            .into_iter()
            .map(|d| WebhookDeliveryResponse {
                id: d.id.map(|i| i.to_hex()).unwrap_or_default(),
                delivery_id: d.delivery_id,
                event: d.event,
                status: d.status,
                attempts: d.attempts,
                next_attempt_at: d.next_attempt_at.to_rfc3339(),
                last_status_code: d.last_status_code,
                last_error: d.last_error,
                created_at: d.created_at.to_rfc3339(),
                delivered_at: d.delivered_at.map(|t| t.to_rfc3339()),
            })
            .collect(),
    ))
}

pub async fn redeliver(
    State(service): State<WebhookService>,
    claims: Claims,
    Path((id, delivery_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    service
        .redeliver(parse_id(&id, "webhook id")?, parse_id(&delivery_id, "delivery id")?, user_id(&claims)?)
        .await?;
    Ok(Json(json!({ "message": "Delivery queued" })))
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
