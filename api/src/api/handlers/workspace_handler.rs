use crate::dtos::workspace_dtos::{
    AcceptInviteRequest, CommentsQuery, CreateCommentRequest, CreateWorkspaceRequest, InviteMemberRequest,
    InvitePreviewQuery, UpdateMemberRoleRequest, UpdateWorkspaceRequest,
};
use crate::services::workspace_service::WorkspaceService;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{
    extract::{Path, Query, State},
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

fn uid(claims: &Claims) -> Result<ObjectId, AppError> {
    ObjectId::from_str(&claims.sub).map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()))
}

fn oid(raw: &str, what: &str) -> Result<ObjectId, AppError> {
    ObjectId::from_str(raw).map_err(|_| AppError::BadRequest(format!("Invalid {what}")))
}

fn to_json<T: serde::Serialize>(value: T) -> Result<Json<Value>, AppError> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|_| AppError::InternalError("Serialization failed".into()))
}

// --- Members and invitations ---

pub async fn list_members(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    to_json(service.list_members(oid(&id, "workspace ID")?, uid(&claims)?).await?)
}

pub async fn invite_member(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<InviteMemberRequest>,
) -> Result<Json<Value>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    to_json(
        service
            .invite_member(oid(&id, "workspace ID")?, uid(&claims)?, &claims.email, &payload.email, &payload.role)
            .await?,
    )
}

pub async fn update_member_role(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path((id, member_id)): Path<(String, String)>,
    Json(payload): Json<UpdateMemberRoleRequest>,
) -> Result<Json<Value>, AppError> {
    service
        .set_member_role(oid(&id, "workspace ID")?, uid(&claims)?, oid(&member_id, "member ID")?, &payload.role)
        .await?;
    Ok(Json(json!({ "message": "Role updated" })))
}

pub async fn remove_member(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path((id, member_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    service
        .remove_member(oid(&id, "workspace ID")?, uid(&claims)?, oid(&member_id, "member ID")?)
        .await?;
    Ok(Json(json!({ "message": "Member removed" })))
}

pub async fn leave_workspace(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    service.leave(oid(&id, "workspace ID")?, uid(&claims)?).await?;
    Ok(Json(json!({ "message": "You left the workspace" })))
}

/// Unauthenticated on purpose: the invitee may not have an account yet. The
/// unguessable token is the credential, and it reveals only the workspace name,
/// the invited address and the role.
pub async fn preview_invite(
    State(service): State<WorkspaceService>,
    Query(query): Query<InvitePreviewQuery>,
) -> Result<Json<Value>, AppError> {
    to_json(service.preview_invite(query.token.trim()).await?)
}

pub async fn accept_invite(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Json(payload): Json<AcceptInviteRequest>,
) -> Result<Json<Value>, AppError> {
    let workspace = service.accept_invite(payload.token.trim(), uid(&claims)?, &claims.email).await?;
    to_json(workspace)
}

// --- Comments ---

pub async fn list_comments(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
    Query(query): Query<CommentsQuery>,
) -> Result<Json<Value>, AppError> {
    to_json(service.list_comments(oid(&id, "workspace ID")?, uid(&claims)?, &query.target).await?)
}

pub async fn add_comment(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<CreateCommentRequest>,
) -> Result<Json<Value>, AppError> {
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;
    to_json(
        service
            .add_comment(oid(&id, "workspace ID")?, uid(&claims)?, &claims.email, payload.target_id, payload.body)
            .await?,
    )
}

pub async fn delete_comment(
    State(service): State<WorkspaceService>,
    claims: Claims,
    Path((id, comment_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    service
        .delete_comment(oid(&id, "workspace ID")?, uid(&claims)?, oid(&comment_id, "comment ID")?)
        .await?;
    Ok(Json(json!({ "message": "Comment deleted" })))
}
