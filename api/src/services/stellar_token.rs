//! Decodes SEP-41 token events (`transfer`, `mint`, `burn`, `clawback`) into
//! balance movements. Only the topics and value are read; the token's own
//! contract id is attached by the caller.
//!
//! Layouts (SEP-41 and the Stellar Asset Contract):
//! - transfer: topics `[sym "transfer", from, to, (asset?)]`, data `i128` or
//!   `{ amount, to_muxed_id }`
//! - mint:     topics `[sym "mint", admin?, to]`, data amount
//! - burn:     topics `[sym "burn", from]`, data amount
//! - clawback: topics `[sym "clawback", admin, from]`, data amount

use stellar_xdr::{AccountId, ContractId, Limits, PublicKey, ReadXdr, ScAddress, ScVal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTransfer {
    pub kind: &'static str,
    pub from: Option<String>,
    pub to: Option<String>,
    pub amount: i128,
}

fn scval(b64: &str) -> Option<ScVal> {
    ScVal::from_xdr_base64(b64, Limits::none()).ok()
}

fn address_string(address: &ScAddress) -> Option<String> {
    match address {
        ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(key))) => {
            Some(stellar_strkey::ed25519::PublicKey(key.0).to_string())
        }
        ScAddress::Contract(ContractId(hash)) => Some(stellar_strkey::Contract(hash.0).to_string()),
        _ => None,
    }
}

fn i128_of(value: &ScVal) -> Option<i128> {
    match value {
        ScVal::I128(parts) => Some(((parts.hi as i128) << 64) | (parts.lo as i128)),
        _ => None,
    }
}

/// Amount from the event data: a bare `i128`, or a map carrying an `amount` key.
fn amount_of(value: &ScVal) -> Option<i128> {
    if let Some(amount) = i128_of(value) {
        return Some(amount);
    }
    if let ScVal::Map(Some(map)) = value {
        for entry in map.0.iter() {
            if let ScVal::Symbol(key) = &entry.key {
                if key.0.as_slice() == b"amount" {
                    return i128_of(&entry.val);
                }
            }
        }
    }
    None
}

/// Returns the movement for a token event, or None when the event is not a
/// SEP-41 movement (other contract events are ignored).
pub fn decode_transfer(topics: &[String], value: Option<&str>) -> Option<DecodedTransfer> {
    let decoded: Vec<ScVal> = topics.iter().map(|t| scval(t)).collect::<Option<Vec<_>>>()?;
    let name = match decoded.first()? {
        ScVal::Symbol(symbol) => std::str::from_utf8(symbol.0.as_slice()).ok()?.to_string(),
        _ => return None,
    };
    let addresses: Vec<String> = decoded[1..]
        .iter()
        .filter_map(|v| match v {
            ScVal::Address(a) => address_string(a),
            _ => None,
        })
        .collect();
    let amount = amount_of(&scval(value?)?)?;

    match name.as_str() {
        "transfer" if addresses.len() >= 2 => Some(DecodedTransfer {
            kind: "transfer",
            from: Some(addresses[0].clone()),
            to: Some(addresses[1].clone()),
            amount,
        }),
        "mint" => Some(DecodedTransfer {
            kind: "mint",
            from: None,
            to: addresses.last().cloned(),
            amount,
        })
        .filter(|t| t.to.is_some()),
        "burn" => Some(DecodedTransfer {
            kind: "burn",
            from: addresses.last().cloned(),
            to: None,
            amount,
        })
        .filter(|t| t.from.is_some()),
        "clawback" => Some(DecodedTransfer {
            kind: "clawback",
            from: addresses.last().cloned(),
            to: None,
            amount,
        })
        .filter(|t| t.from.is_some()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{Hash, Int128Parts, ScSymbol, Uint256, WriteXdr};

    fn b64(v: &ScVal) -> String {
        v.to_xdr_base64(Limits::none()).unwrap()
    }

    fn account(byte: u8) -> ScVal {
        ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([byte; 32])))))
    }

    fn sym(s: &str) -> ScVal {
        ScVal::Symbol(ScSymbol(s.as_bytes().try_into().unwrap()))
    }

    fn i128_val(n: i128) -> ScVal {
        ScVal::I128(Int128Parts { hi: (n >> 64) as i64, lo: n as u64 })
    }

    #[test]
    fn decodes_transfer_with_bare_amount() {
        let topics = vec![b64(&sym("transfer")), b64(&account(1)), b64(&account(2))];
        let decoded = decode_transfer(&topics, Some(&b64(&i128_val(5_000_000)))).expect("transfer");
        assert_eq!(decoded.kind, "transfer");
        assert_eq!(decoded.amount, 5_000_000);
        assert_eq!(decoded.from, Some(stellar_strkey::ed25519::PublicKey([1; 32]).to_string()));
        assert_eq!(decoded.to, Some(stellar_strkey::ed25519::PublicKey([2; 32]).to_string()));
    }

    #[test]
    fn decodes_transfer_with_muxed_amount_map() {
        let entry = |k: &str, v: ScVal| stellar_xdr::ScMapEntry { key: sym(k), val: v };
        let map = ScVal::Map(Some(stellar_xdr::ScMap(
            vec![entry("amount", i128_val(42)), entry("to_muxed_id", ScVal::U64(7))].try_into().unwrap(),
        )));
        let topics = vec![b64(&sym("transfer")), b64(&account(1)), b64(&account(2))];
        let decoded = decode_transfer(&topics, Some(&b64(&map))).expect("transfer");
        assert_eq!(decoded.amount, 42);
    }

    #[test]
    fn burn_uses_last_address_as_holder() {
        let topics = vec![b64(&sym("burn")), b64(&account(9))];
        let decoded = decode_transfer(&topics, Some(&b64(&i128_val(3)))).expect("burn");
        assert_eq!(decoded.kind, "burn");
        assert_eq!(decoded.from, Some(stellar_strkey::ed25519::PublicKey([9; 32]).to_string()));
        assert_eq!(decoded.to, None);
    }

    #[test]
    fn ignores_unrelated_events() {
        let topics = vec![b64(&sym("approve")), b64(&account(1)), b64(&account(2))];
        assert!(decode_transfer(&topics, Some(&b64(&i128_val(1)))).is_none());
    }

    #[test]
    fn contract_addresses_encode_as_c_strkey() {
        let hash = Hash([3; 32]);
        let topics = vec![
            b64(&sym("transfer")),
            b64(&ScVal::Address(ScAddress::Contract(ContractId(hash.clone())))),
            b64(&account(2)),
        ];
        let decoded = decode_transfer(&topics, Some(&b64(&i128_val(1)))).expect("transfer");
        assert!(decoded.from.unwrap().starts_with('C'));
    }
}
