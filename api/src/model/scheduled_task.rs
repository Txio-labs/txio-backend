use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use validator::Validate;

/// A trigger condition a scheduled task waits for before executing. Recurring
/// and one-off time triggers are self-contained; PriceThreshold needs a price
/// read at evaluation time (source depends on the pair — chain RPC for
/// on-chain reference prices, or an external feed — decided per task, not
/// hardcoded here).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerKind {
    Recurring { interval_minutes: u32 },
    TimeOnce { at: DateTime<Utc> },
    PriceThreshold {
        chain: String,
        token: String,
        above: bool,
        value_usd: f64,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledTaskStatus {
    Active,
    Paused,
    Cancelled,
}

/// A recurring or conditional transaction, executed unattended by the
/// backend worker via the linked session key — never with a raw private key
/// stored server-side. Every execution goes through the same spend-policy
/// gate an interactive transaction does.
#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct ScheduledTask {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,
    pub session_key_id: ObjectId,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    pub trigger: TriggerKind,

    /// The TRANSACTION request to run when triggered — same opaque
    /// chain-native shape History's tx_params already uses, replayed as-is.
    pub request_template: Value,

    pub status: ScheduledTaskStatus,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub next_run_at: Option<DateTime<Utc>>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub last_run_at: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl ScheduledTask {
    pub fn new(
        user_id: ObjectId,
        session_key_id: ObjectId,
        name: String,
        trigger: TriggerKind,
        request_template: Value,
    ) -> Self {
        let next_run_at = match &trigger {
            TriggerKind::Recurring { interval_minutes } => {
                Some(Utc::now() + chrono::Duration::minutes(*interval_minutes as i64))
            }
            TriggerKind::TimeOnce { at } => Some(*at),
            // Evaluated by the worker on every poll instead of a fixed time.
            TriggerKind::PriceThreshold { .. } => None,
        };

        Self {
            id: None,
            user_id,
            session_key_id,
            name,
            trigger,
            request_template,
            status: ScheduledTaskStatus::Active,
            next_run_at,
            last_run_at: None,
            last_error: None,
            created_at: Utc::now(),
        }
    }
}
