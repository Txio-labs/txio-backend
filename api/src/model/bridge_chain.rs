use serde::{Deserialize, Serialize};

/// Chain-agnostic identifier. Any chain the workspace supports (or that a
/// provider adds later) is representable without a code change — no chain
/// is special-cased as "primary".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainFamily {
    Evm,
    Sui,
    Solana,
    Aptos,
    Stellar,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainAsset {
    /// e.g. "ethereum", "polygon", "sui", "stellar", "solana" — provider-specific slug.
    pub chain: String,
    pub family: ChainFamily,
    /// Token contract/denom address, or "native".
    pub token: String,
    pub symbol: String,
    pub decimals: u8,
}
