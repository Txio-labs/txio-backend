use chrono::{DateTime, Utc};
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateSessionKeyRequest {
    pub wallet_family: String,

    #[validate(length(min = 1, message = "Wallet address cannot be empty"))]
    pub wallet_address: String,

    #[validate(length(min = 1, message = "Label cannot be empty"))]
    pub label: String,

    #[validate(length(min = 1, message = "Delegate address cannot be empty"))]
    pub delegate_address: String,

    /// The ephemeral signer's raw private key, generated client-side and
    /// sent once (over TLS) so the backend can encrypt and store it —
    /// required because unattended scheduled execution has no human to
    /// prompt for a signature. Never returned in any response after this.
    #[validate(length(min = 1, message = "Delegate private key cannot be empty"))]
    pub delegate_private_key: String,

    #[serde(default)]
    pub scoped_contracts: Vec<String>,

    pub max_amount_per_tx_usd: Option<f64>,

    pub expires_at: DateTime<Utc>,
}
