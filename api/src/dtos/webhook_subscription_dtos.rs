use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateWebhookRequest {
    #[validate(length(min = 1, message = "URL cannot be empty"))]
    pub url: String,

    #[validate(length(min = 1, message = "Select at least one event"))]
    pub events: Vec<String>,
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
