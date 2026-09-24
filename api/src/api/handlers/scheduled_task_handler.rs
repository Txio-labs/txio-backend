use crate::dtos::scheduled_task_dtos::CreateScheduledTaskRequest;
use crate::model::scheduled_task::ScheduledTaskStatus;
use crate::services::scheduled_task_service::ScheduledTaskService;
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

pub async fn create_task(
    State(service): State<ScheduledTaskService>,
    claims: Claims,
    Json(payload): Json<CreateScheduledTaskRequest>,
) -> Result<Json<Value>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    let task = service.create(user_id(&claims)?, payload).await?;
    Ok(Json(serde_json::to_value(task).unwrap()))
}

pub async fn list_tasks(
    State(service): State<ScheduledTaskService>,
    claims: Claims,
) -> Result<Json<Value>, AppError> {
    let tasks = service.list(user_id(&claims)?).await?;
    Ok(Json(serde_json::to_value(tasks).unwrap()))
}

pub async fn pause_task(
    State(service): State<ScheduledTaskService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let task_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid task id".into()))?;
    service.set_status(task_id, user_id(&claims)?, ScheduledTaskStatus::Paused).await?;
    Ok(Json(json!({ "message": "Task paused" })))
}

pub async fn resume_task(
    State(service): State<ScheduledTaskService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let task_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid task id".into()))?;
    service.set_status(task_id, user_id(&claims)?, ScheduledTaskStatus::Active).await?;
    Ok(Json(json!({ "message": "Task resumed" })))
}

pub async fn cancel_task(
    State(service): State<ScheduledTaskService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let task_id = ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid task id".into()))?;
    service.set_status(task_id, user_id(&claims)?, ScheduledTaskStatus::Cancelled).await?;
    Ok(Json(json!({ "message": "Task cancelled" })))
}
