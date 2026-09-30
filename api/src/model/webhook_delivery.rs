use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_IN_FLIGHT: &str = "in_flight";
pub const STATUS_DELIVERED: &str = "delivered";
pub const STATUS_DEAD: &str = "dead";

/// One event queued for one subscription. Stored durably so a restart, a slow
/// receiver or a crash mid-attempt does not lose it; the worker retries with
/// backoff until it is delivered or gives up (`dead`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebhookDelivery {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub subscription_id: ObjectId,
    pub user_id: ObjectId,
    pub event: String,
    /// Stable across retries and redeliveries; receivers dedupe on it
    /// (`X-Txio-Delivery`).
    pub delivery_id: String,
    /// The exact JSON body that is signed and sent.
    pub body: String,
    pub attempts: u32,
    pub status: String,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub next_attempt_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub delivered_at: Option<DateTime<Utc>>,
}

impl WebhookDelivery {
    pub fn new(subscription_id: ObjectId, user_id: ObjectId, event: String, delivery_id: String, body: String) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            subscription_id,
            user_id,
            event,
            delivery_id,
            body,
            attempts: 0,
            status: STATUS_PENDING.to_string(),
            next_attempt_at: now,
            last_status_code: None,
            last_error: None,
            created_at: now,
            delivered_at: None,
        }
    }
}
