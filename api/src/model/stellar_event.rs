use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One Soroban contract event, stored by the indexer so a `C...` address can
/// show history beyond the RPC's ~7-day retention window. `_id` is
/// `{network}:{rpc event id}` so event ids from different networks never clash.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StellarContractEvent {
    #[serde(rename = "_id")]
    pub id: String,

    pub network: String,
    pub contract_id: String,
    pub ledger: i64,
    pub ledger_closed_at: Option<DateTime<Utc>>,
    pub tx_hash: String,
    pub event_type: String,
    /// Base64 XDR `ScVal`s, exactly as the RPC returned them.
    pub topics: Vec<String>,
    /// Base64 XDR `ScVal`, exactly as the RPC returned it.
    pub value: Option<String>,
    pub in_successful_contract_call: bool,
}

/// A contract the indexer tracks. Added on first lookup; `backfilled` flips
/// once the retained RPC window has been read in full.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StellarContractWatch {
    #[serde(rename = "_id")]
    pub key: String,

    pub network: String,
    pub contract_id: String,
    pub backfilled: bool,
    /// Highest ledger already read for this contract.
    pub last_ledger: i64,
    pub added_at: DateTime<Utc>,
}

/// Response shape for the lookup endpoint; serialized to camelCase for the frontend.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StellarContractEventDto {
    pub tx_hash: String,
    pub ledger: i64,
    pub ledger_closed_at: Option<DateTime<Utc>>,
    pub event_type: String,
    pub topics: Vec<String>,
    pub value: Option<String>,
    pub in_successful_contract_call: bool,
}

impl From<StellarContractEvent> for StellarContractEventDto {
    fn from(e: StellarContractEvent) -> Self {
        Self {
            tx_hash: e.tx_hash,
            ledger: e.ledger,
            ledger_closed_at: e.ledger_closed_at,
            event_type: e.event_type,
            topics: e.topics,
            value: e.value,
            in_successful_contract_call: e.in_successful_contract_call,
        }
    }
}

/// Pulls the base64 XDR out of an RPC field that is either a plain string
/// (older RPC) or an object `{ "xdr": "..." }` (newer RPC).
pub fn xdr_of(raw: &Value) -> Option<String> {
    match raw {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => map.get("xdr").and_then(|v| v.as_str()).map(str::to_string),
        _ => None,
    }
}

/// One decoded SEP-41 movement from a successful token event. `_id` is the
/// source event's id, so re-reading a ledger never double-counts a transfer.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StellarTokenTransfer {
    #[serde(rename = "_id")]
    pub id: String,

    pub network: String,
    pub token_contract: String,
    /// "transfer" | "mint" | "burn" | "clawback"
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Decimal string of the i128 amount in the token's base units.
    pub amount: String,
    pub ledger: i64,
}

/// Net balance of one holder in one token, derived from indexed transfers.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TokenBalance {
    pub token_contract: String,
    /// Raw i128 base units as a decimal string.
    pub balance: String,
    pub transfer_count: u64,
}

/// Folds transfers into per-token net balances for `holder`, largest first.
/// Zero balances are dropped. Overflow on an i128 sum is skipped, not wrapped.
pub fn aggregate_balances(holder: &str, transfers: &[StellarTokenTransfer]) -> Vec<TokenBalance> {
    use std::collections::BTreeMap;
    let mut by_token: BTreeMap<String, (i128, u64)> = BTreeMap::new();
    for t in transfers {
        let Ok(amount) = t.amount.parse::<i128>() else { continue };
        let entry = by_token.entry(t.token_contract.clone()).or_insert((0, 0));
        let mut touched = false;
        if t.from.as_deref() == Some(holder) {
            if let Some(next) = entry.0.checked_sub(amount) {
                entry.0 = next;
                touched = true;
            }
        }
        if t.to.as_deref() == Some(holder) {
            if let Some(next) = entry.0.checked_add(amount) {
                entry.0 = next;
                touched = true;
            }
        }
        if touched {
            entry.1 += 1;
        }
    }
    let mut out: Vec<(String, i128, u64)> = by_token
        .into_iter()
        .filter(|(_, (balance, _))| *balance != 0)
        .map(|(token, (balance, count))| (token, balance, count))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out.into_iter()
        .map(|(token_contract, balance, transfer_count)| TokenBalance {
            token_contract,
            balance: balance.to_string(),
            transfer_count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(token: &str, from: Option<&str>, to: Option<&str>, amount: &str) -> StellarTokenTransfer {
        StellarTokenTransfer {
            id: format!("n:{amount}"),
            network: "testnet".into(),
            token_contract: token.into(),
            kind: "transfer".into(),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
            amount: amount.into(),
            ledger: 1,
        }
    }

    #[test]
    fn nets_incoming_and_outgoing_per_token() {
        let transfers = vec![
            t("T1", None, Some("ME"), "100"),
            t("T1", Some("ME"), Some("OTHER"), "30"),
            t("T2", Some("OTHER"), Some("ME"), "5"),
            t("T2", Some("ME"), Some("OTHER"), "5"),
        ];
        let balances = aggregate_balances("ME", &transfers);
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].token_contract, "T1");
        assert_eq!(balances[0].balance, "70");
        assert_eq!(balances[0].transfer_count, 2);
    }

    #[test]
    fn self_transfer_is_net_zero() {
        let transfers = vec![t("T1", Some("ME"), Some("ME"), "9")];
        assert!(aggregate_balances("ME", &transfers).is_empty());
    }

    #[test]
    fn sorts_largest_balance_first() {
        let transfers = vec![t("SMALL", None, Some("ME"), "1"), t("BIG", None, Some("ME"), "1000")];
        let balances = aggregate_balances("ME", &transfers);
        assert_eq!(balances[0].token_contract, "BIG");
    }
}
