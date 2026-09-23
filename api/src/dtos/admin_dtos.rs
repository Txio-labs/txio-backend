use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminUsersResponse {
    pub emails: Vec<String>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct AdminDeleteUserRequest {
    #[validate(email)]
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminStatsResponse {
    pub user_count: u64,
    pub rpc_log_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminLogEntry {
    pub method: String,
    pub success: bool,
    pub error: Option<String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminOverviewResponse {
    pub users: u64,
    pub admins: u64,
    pub workspaces: u64,
    pub collections: u64,
    pub saved_requests: u64,
    pub history_entries: u64,
    pub rpc_logs: u64,
    pub active_sessions: u64,
    pub signups_last_7d: u64,
    pub requests_last_24h: u64,
    pub rpc_calls_last_24h: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminUserEntry {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub created_at: Option<String>,
    pub is_admin: bool,
    pub google_linked: bool,
    pub github_login: Option<String>,
    pub tier: Option<String>,
    pub collection_count: u64,
    pub request_count: u64,
    pub last_active_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminRequestEntry {
    pub id: String,
    pub user_email: Option<String>,
    pub name: String,
    pub request_type: Option<String>,
    pub chain: Option<String>,
    pub network: Option<String>,
    pub method: Option<String>,
    pub status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub executed_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminCollectionEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_email: Option<String>,
    pub request_count: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}
