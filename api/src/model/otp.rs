use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OTP {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub email: String,
    pub otp: String,
    #[serde(default)]
    pub failed_attempts: i32,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    /// Set to `true` when the failed-attempt cap is reached. A locked row is
    /// not deleted immediately; it stays in the collection until the TTL index
    /// removes it so that `generate_otp` can still read `created_at` and
    /// enforce the resend cooldown, even after cap-out.
    #[serde(default)]
    pub locked: bool,
}

impl OTP {
    pub fn new(email: String, otp: String) -> Self {
        Self {
            id: None,
            email,
            otp,
            failed_attempts: 0,
            created_at: Utc::now(),
            locked: false,
        }
    }
}
