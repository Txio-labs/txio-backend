use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateWebhookRequest {
    #[validate(length(min = 1, message = "URL cannot be empty"))]
    pub url: String,

    #[validate(length(min = 1, message = "Select at least one event"))]
    pub events: Vec<String>,
}

/// A subscription as returned by the API. Secret material (the bcrypt hash and
/// the encrypted secret) is never included.
#[derive(Debug, Serialize)]
pub struct WebhookSubscriptionResponse {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub is_active: bool,
    /// False for subscriptions created before secrets were recoverable; their
    /// deliveries cannot be signed until the secret is rotated.
    pub can_sign: bool,
    pub last_delivered_at: Option<String>,
    pub last_delivery_error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct WebhookDeliveryResponse {
    pub id: String,
    pub delivery_id: String,
    pub event: String,
    pub status: String,
    pub attempts: u32,
    pub next_attempt_at: String,
    pub last_status_code: Option<u16>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub delivered_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RotateSecretResponse {
    pub secret: String,
}

/// The raw secret is only ever returned here, at creation — afterwards only
/// `secret_hash` is stored, matching how a new API key or password is shown
/// once and never retrievable again.
#[derive(Debug, Serialize)]
pub struct CreateWebhookResponse {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
}
