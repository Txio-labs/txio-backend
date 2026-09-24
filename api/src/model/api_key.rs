use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use validator::Validate;

/// Scopes an API key can be granted — a fixed set, same rationale as
/// webhook_subscription's WEBHOOK_EVENTS: keeps issuing code and any future
/// scope-check code in sync with what's actually enforced.
pub const API_KEY_SCOPES: &[&str] = &["history:read", "routes:read", "transactions:simulate", "transactions:execute"];

/// A public-API credential. The raw key is shown exactly once, at creation
/// (standard API-key UX) — only its SHA-256 hash is ever stored, looked up
/// on every request by the `ApiKeyAuth` extractor (see utils::api_key_auth).
/// SHA-256 rather than bcrypt: this hash is checked on every public API
/// call, so it needs to be fast, and — unlike a user password — the raw key
/// is high-entropy and randomly generated, not guessable, so bcrypt's
/// slow-by-design property buys nothing here.
#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct ApiKey {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,

    #[validate(length(min = 1, message = "Label cannot be empty"))]
    pub label: String,

    /// Never serialized in any API response — see ApiKeyAuth, the only
    /// consumer, which reads this field directly on the in-memory struct.
    #[serde(skip_serializing)]
    pub key_hash: String,

    /// A short, non-secret prefix of the raw key (e.g. `txio_live_ab12`),
    /// stored alongside the hash so a user can recognize which key is which
    /// in a list without Txio ever holding the full raw value again.
    pub key_prefix: String,

    #[serde(default)]
    pub scopes: Vec<String>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub last_used_at: Option<DateTime<Utc>>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub revoked_at: Option<DateTime<Utc>>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl ApiKey {
    pub fn new(user_id: ObjectId, label: String, key_hash: String, key_prefix: String, scopes: Vec<String>) -> Self {
        Self {
            id: None,
            user_id,
            label,
            key_hash,
            key_prefix,
            scopes,
            last_used_at: None,
            revoked_at: None,
            created_at: Utc::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}
