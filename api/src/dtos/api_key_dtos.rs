use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateApiKeyRequest {
    #[validate(length(min = 1, message = "Label cannot be empty"))]
    pub label: String,

    #[validate(length(min = 1, message = "Select at least one scope"))]
    pub scopes: Vec<String>,
}

/// The raw key is only ever present in this one response, at creation.
#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub label: String,
    pub key_prefix: String,
    pub scopes: Vec<String>,
    pub key: String,
}
