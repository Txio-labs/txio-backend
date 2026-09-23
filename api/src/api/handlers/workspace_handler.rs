use crate::dtos::workspace_dtos::{CreateWorkspaceRequest, UpdateWorkspaceRequest};
use crate::services::workspace_service::WorkspaceService;
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

pub async fn create_workspace(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Json(payload): Json<CreateWorkspaceRequest>,
) -> Result<Json<Value>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;

    let workspace = service
        .create_workspace(
            user_id,
            payload.name,
            payload.workspace_type.unwrap_or_default(),
        )
        .await?;

    Ok(Json(serde_json::to_value(workspace).unwrap()))
}

pub async fn get_user_workspaces(
    State(service): State<WorkspaceService>,
    claims: Claims,
) -> Result<Json<Value>, AppError> {
    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;

    let workspaces = service.get_user_workspaces(user_id).await?;

    Ok(Json(serde_json::to_value(workspaces).unwrap()))
}

pub async fn update_workspace(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<UpdateWorkspaceRequest>,
) -> Result<Json<Value>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let workspace_id =
        ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid workspace ID".into()))?;

    let workspace = service
        .rename_workspace(workspace_id, user_id, payload.name)
        .await?;

    Ok(Json(serde_json::to_value(workspace).unwrap()))
}

pub async fn delete_workspace(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))?;
    let workspace_id =
        ObjectId::from_str(&id).map_err(|_| AppError::BadRequest("Invalid workspace ID".into()))?;

    service.delete_workspace(workspace_id, user_id).await?;

    Ok(Json(
        json!({ "message": "Workspace deleted successfully" }),
    ))
}
