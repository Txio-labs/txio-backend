use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// The chains the public API accepts for address lookups and labels.
pub const LOOKUP_CHAINS: &[&str] = &["sui", "evm", "solana", "aptos", "stellar", "starknet"];

pub const MAX_LABEL_LENGTH: usize = 80;

/// A label one user attached to an address on one chain. Labels are private to
/// the user who saved them; no other user can read them through the API.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AddressLabel {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub user_id: ObjectId,
    pub chain: String,
    /// Stored through `normalize_address`, so lookups match regardless of hex casing.
    pub address: String,
    pub label: String,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

/// Hex addresses (EVM, Sui, Aptos, Starknet) are case-insensitive, so they are
/// lowercased. Base58 (Solana) and base32 (Stellar) addresses are case-sensitive
/// and are kept as given.
pub fn normalize_address(address: &str) -> String {
    let trimmed = address.trim();
    if trimmed.len() > 2 && (trimmed.starts_with("0x") || trimmed.starts_with("0X")) {
        format!("0x{}", trimmed[2..].to_lowercase())
    } else {
        trimmed.to_string()
    }
}

/// Returns the trimmed label, or the reason it was rejected.
pub fn validate_label(label: &str) -> Result<String, String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err("label must not be empty".into());
    }
    if trimmed.chars().count() > MAX_LABEL_LENGTH {
        return Err(format!("label must be at most {MAX_LABEL_LENGTH} characters"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("label must not contain control characters".into());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_addresses_are_lowercased_and_prefixed() {
        assert_eq!(normalize_address("0xABCdef"), "0xabcdef");
        assert_eq!(normalize_address("  0XABC  "), "0xabc");
        // Unprefixed text may be base58 (Solana), so it is never case-folded.
        assert_eq!(normalize_address("ABCDEF12"), "ABCDEF12");
    }

    #[test]
    fn case_sensitive_addresses_are_kept() {
        let sol = "7EcDhSYGxXyscszYEp35KHN8vvw3svAuLKTzXwCFLtV";
        assert_eq!(normalize_address(sol), sol);
        let xlm = "GD6ERGKBPEZJGS2DJ2N24RIPX3F3BPJ4UUBCJLS763L2JXBHJQWVU6KZ";
        assert_eq!(normalize_address(xlm), xlm);
    }

    #[test]
    fn labels_are_trimmed_and_bounded() {
        assert_eq!(validate_label("  Exchange hot wallet ").unwrap(), "Exchange hot wallet");
        assert!(validate_label("   ").is_err());
        assert!(validate_label(&"x".repeat(MAX_LABEL_LENGTH + 1)).is_err());
        assert!(validate_label("bad\u{0007}bell").is_err());
    }
}
