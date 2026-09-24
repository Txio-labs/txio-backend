use crate::dtos::admin_dtos::{
    AdminCollectionEntry, AdminDeleteUserRequest, AdminEndpointStatsEntry, AdminLogEntry,
    AdminOverviewResponse, AdminRequestEntry, AdminStatsResponse, AdminUserEntry,
    AdminUsersResponse,
};
use crate::services::admin_service::AdminService;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use validator::Validate;

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub limit: Option<i64>,
}

const DEFAULT_LOG_LIMIT: i64 = 20;
const MAX_LOG_LIMIT: i64 = 200;
const DEFAULT_LIST_LIMIT: i64 = 100;
const MAX_LIST_LIMIT: i64 = 500;

fn list_limit(query: &LogsQuery) -> i64 {
    query
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT)
}

pub async fn list_users(
    State(service): State<AdminService>,
    claims: Claims,
) -> Result<Json<AdminUsersResponse>, AppError> {
    let emails = service.list_user_emails(&claims).await?;
    Ok(Json(AdminUsersResponse { emails }))
}

pub async fn delete_user(
    State(service): State<AdminService>,
    claims: Claims,
    Json(payload): Json<AdminDeleteUserRequest>,
) -> Result<Json<Value>, AppError> {
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let deleted_email = service.delete_user(&claims, &payload.email).await?;

    Ok(Json(
        json!({ "message": "User deleted", "email": deleted_email }),
    ))
}

pub async fn stats(
    State(service): State<AdminService>,
    claims: Claims,
) -> Result<Json<AdminStatsResponse>, AppError> {
    let stats = service.stats(&claims).await?;
    Ok(Json(stats))
}

pub async fn list_logs(
    State(service): State<AdminService>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<AdminLogEntry>>, AppError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LOG_LIMIT)
        .clamp(1, MAX_LOG_LIMIT);
    let logs = service.list_logs(&claims, limit).await?;
    Ok(Json(logs))
}

const DEFAULT_ENDPOINT_STATS_SAMPLE: i64 = 1000;
const MAX_ENDPOINT_STATS_SAMPLE: i64 = 5000;

pub async fn endpoint_stats(
    State(service): State<AdminService>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<AdminEndpointStatsEntry>>, AppError> {
    let sample_size = query
        .limit
        .unwrap_or(DEFAULT_ENDPOINT_STATS_SAMPLE)
        .clamp(1, MAX_ENDPOINT_STATS_SAMPLE);
    let stats = service.endpoint_stats(&claims, sample_size).await?;
    Ok(Json(stats))
}

pub async fn overview(
    State(service): State<AdminService>,
    claims: Claims,
) -> Result<Json<AdminOverviewResponse>, AppError> {
    Ok(Json(service.overview(&claims).await?))
}

pub async fn list_accounts(
    State(service): State<AdminService>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<AdminUserEntry>>, AppError> {
    Ok(Json(service.list_accounts(&claims, list_limit(&query)).await?))
}

pub async fn list_requests(
    State(service): State<AdminService>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<AdminRequestEntry>>, AppError> {
    Ok(Json(service.recent_requests(&claims, list_limit(&query)).await?))
}

pub async fn list_collections(
    State(service): State<AdminService>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<AdminCollectionEntry>>, AppError> {
    Ok(Json(service.list_collections(&claims, list_limit(&query)).await?))
}
