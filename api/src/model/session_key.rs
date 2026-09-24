use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use validator::Validate;

/// A delegated-signing scope for automations to act on a user's behalf. The
/// real wallet's private key is never seen by Txio — instead the real
/// wallet authorizes a locally-generated *ephemeral* keypair (via an
/// on-chain delegation transaction, e.g. an ERC-20 `approve`-style grant on
/// EVM, or Sui's native sponsored/session-transaction primitive) for a
/// specific contract, up to an amount, until expiry.
///
/// The ephemeral key's private key IS held server-side (encrypted at rest,
/// see utils::session_key_crypto) — scheduled/conditional execution has no
/// human present to prompt for a signature, so the backend must be able to
/// sign with it directly. This is a deliberate, scoped exception to "Txio
/// never holds a private key": the blast radius of a leak is capped by this
/// key's own `scoped_contracts`/`max_amount_per_tx_usd`/`expires_at`, unlike
/// the user's real wallet key, which is never generated, transmitted, or
/// stored here under any circumstance.
#[derive(Debug, Serialize, Deserialize, Validate, Clone)]
pub struct SessionKey {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    pub user_id: ObjectId,

    pub wallet_family: String,

    #[validate(length(min = 1, message = "Wallet address cannot be empty"))]
    pub wallet_address: String,

    #[validate(length(min = 1, message = "Label cannot be empty"))]
    pub label: String,

    /// The ephemeral key's own public address — what on-chain grants
    /// reference, distinct from `wallet_address` (the real, delegating wallet).
    pub delegate_address: String,

    /// `session_key_crypto::encrypt`-produced ciphertext of the ephemeral
    /// signer's private key. Never serialized back to any API response —
    /// see `#[serde(skip_serializing)]` below — only read internally by the
    /// scheduler worker at execution time.
    #[serde(skip_serializing)]
    pub encrypted_private_key: String,

    /// Empty means not contract-restricted. Discouraged (a session key
    /// should normally be scoped) but not blocked, since some automations
    /// legitimately need broader reach.
    #[serde(default)]
    pub scoped_contracts: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_amount_per_tx_usd: Option<f64>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub expires_at: DateTime<Utc>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub revoked_at: Option<DateTime<Utc>>,

    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

impl SessionKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: ObjectId,
        wallet_family: String,
        wallet_address: String,
        label: String,
        delegate_address: String,
        encrypted_private_key: String,
        scoped_contracts: Vec<String>,
        max_amount_per_tx_usd: Option<f64>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: None,
            user_id,
            wallet_family,
            wallet_address,
            label,
            delegate_address,
            encrypted_private_key,
            scoped_contracts,
            max_amount_per_tx_usd,
            expires_at,
            revoked_at: None,
            created_at: Utc::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > Utc::now()
    }
}
