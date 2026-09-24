use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use validator::Validate;

/// Events a webhook subscription can fire on — a fixed set rather than
/// free-form strings, so delivery code and the frontend's subscription UI
/// stay in sync with what the backend actually emits.
pub const WEBHOOK_EVENTS: &[&str] = &[
    "tx.confirmed",
    "tx.failed",
    "swap.resumable",
    "session_key.expiring",
    "spend_limit.reached",
];

#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct WebhookSubscription {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,

    // URL validated in the service layer via utils::url_safety::validate_https_url
    // (shared with collection_service.rs's RPC-URL SSRF checks) before this
    // struct is ever constructed.
    pub url: String,

    #[validate(length(min = 1, message = "Select at least one event"))]
    pub events: Vec<String>,

    /// HMAC signing secret for delivered payloads — generated server-side,
    /// shown once at creation, never returned again (same pattern as an API
    /// key's raw value).
    pub secret_hash: String,

    pub is_active: bool,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub last_delivered_at: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery_error: Option<String>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl WebhookSubscription {
    pub fn new(user_id: ObjectId, url: String, events: Vec<String>, secret_hash: String) -> Self {
        Self {
            id: None,
            user_id,
            url,
            events,
            secret_hash,
            is_active: true,
            last_delivered_at: None,
            last_delivery_error: None,
            created_at: Utc::now(),
        }
    }
}
