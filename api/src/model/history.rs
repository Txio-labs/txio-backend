use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use validator::Validate;

/// One executed request, recorded for the "Request History" audit log.
/// Distinct from `SavedRequest` (collection.rs) — a saved request is a
/// user-curated template kept indefinitely; a history entry is an automatic
/// record of "this ran, here's what happened", capped and prunable.
#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct HistoryEntry {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<ObjectId>,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    /// "RPC" or "TRANSACTION" — mirrors the frontend's RequestType enum.
    pub request_type: String,

    pub chain: Option<String>,
    pub network: String,
    pub method: Option<String>,
    pub params: Option<Value>,

    pub status: i32,
    pub duration_ms: i64,

    pub executed_at: DateTime<Utc>,
}

impl HistoryEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
        name: String,
        request_type: String,
        chain: Option<String>,
        network: String,
        method: Option<String>,
        params: Option<Value>,
        status: i32,
        duration_ms: i64,
    ) -> Self {
        Self {
            id: None,
            user_id,
            workspace_id,
            name,
            request_type,
            chain,
            network,
            method,
            params,
            status,
            duration_ms,
            executed_at: Utc::now(),
        }
    }
}
