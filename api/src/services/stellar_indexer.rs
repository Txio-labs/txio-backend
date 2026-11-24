use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::model::stellar_event::{
    aggregate_balances, xdr_of, StellarContractEvent, StellarContractEventDto, StellarContractWatch,
    StellarTokenTransfer, TokenBalance,
};
use crate::services::stellar_token::decode_transfer;
use crate::repositories::stellar_event_repository::StellarEventRepository;
use crate::utils::error::AppError;

const SYNC_INTERVAL: Duration = Duration::from_secs(20);
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
/// getEvents page size the RPC accepts.
const PAGE_LIMIT: u64 = 200;
/// Upper bound on pages read per contract per tick, so one busy contract
/// cannot starve the others. Remaining pages are read on the next tick.
const MAX_PAGES_PER_TICK: usize = 50;
pub const MAX_HISTORY_LIMIT: i64 = 100;

/// Indexes Soroban contract events for the contracts users look up.
///
/// Soroban RPC only keeps ~7 days of events, so the first sync of a contract
/// reads the whole retained window and later syncs tail new ledgers. Account
/// (`G...`) data is not handled here: Horizon covers it.
#[derive(Clone)]
pub struct StellarIndexer {
    repo: StellarEventRepository,
    http: reqwest::Client,
    rpc_urls: HashMap<String, String>,
}

impl StellarIndexer {
    pub fn new(repo: StellarEventRepository) -> Self {
        let mut rpc_urls = HashMap::new();
        rpc_urls.insert(
            "mainnet".to_string(),
            std::env::var("STELLAR_SOROBAN_RPC_MAINNET")
                .unwrap_or_else(|_| "https://mainnet.sorobanrpc.com".to_string()),
        );
        rpc_urls.insert(
            "testnet".to_string(),
            std::env::var("STELLAR_SOROBAN_RPC_TESTNET")
                .unwrap_or_else(|_| "https://soroban-testnet.stellar.org".to_string()),
        );
        Self {
            repo,
            http: reqwest::Client::new(),
            rpc_urls,
        }
    }

    pub fn supports(&self, network: &str) -> bool {
        self.rpc_urls.contains_key(network)
    }

    /// Stored events for a contract, newest first. Registers the contract for
    /// indexing on first call; `backfilled` is false until the first full read.
    pub async fn history(
        &self,
        network: &str,
        contract_id: &str,
        limit: i64,
    ) -> Result<(Vec<StellarContractEventDto>, bool), AppError> {
        let watch = self.repo.ensure_watch(network, contract_id).await?;
        let limit = limit.clamp(1, MAX_HISTORY_LIMIT);
        let events = self.repo.recent_events(network, contract_id, limit).await?;
        Ok((events.into_iter().map(StellarContractEventDto::from).collect(), watch.backfilled))
    }

    /// Starts the background sync loop. Fire-and-forget, like the scheduler worker.
    pub fn spawn(self) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SYNC_INTERVAL);
            loop {
                interval.tick().await;
                self.tick().await;
            }
        });
    }

    /// Net token balances for a `G...` or `C...` holder, from indexed transfers.
    /// Covers only tokens whose contract has been looked up (and so indexed).
    pub async fn token_balances(&self, network: &str, holder: &str) -> Result<Vec<TokenBalance>, AppError> {
        let transfers = self.repo.transfers_for_holder(network, holder).await?;
        Ok(aggregate_balances(holder, &transfers))
    }

    async fn tick(&self) {
        let watches = match self.repo.all_watches().await {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "stellar indexer: failed to load watch list");
                return;
            }
        };
        for watch in watches {
            if !self.supports(&watch.network) {
                continue;
            }
            if let Err(e) = self.sync(&watch).await {
                tracing::warn!(
                    contract = %watch.contract_id,
                    network = %watch.network,
                    error = %e,
                    "stellar indexer: sync failed; retrying next tick"
                );
            }
        }
    }

    /// Reads new events for one contract and advances its watch progress.
    async fn sync(&self, watch: &StellarContractWatch) -> Result<(), AppError> {
        let url = self
            .rpc_urls
            .get(&watch.network)
            .ok_or_else(|| AppError::InternalError("no Soroban RPC for network".into()))?
            .clone();

        let health = self.rpc(&url, "getHealth", json!({})).await?;
        let oldest = health["oldestLedger"].as_u64().unwrap_or(0) as i64;
        let latest = health["latestLedger"].as_u64().unwrap_or(0) as i64;

        // last_ledger is the highest ledger fully read; resume after it.
        let start = if watch.last_ledger > 0 {
            (watch.last_ledger + 1).max(oldest)
        } else {
            oldest
        };
        if start <= 0 || start > latest {
            return Ok(());
        }

        let filters = json!([{ "type": "contract", "contractIds": [watch.contract_id] }]);
        let mut cursor: Option<String> = None;
        let mut max_ledger_seen = start - 1;
        let mut finished = false;

        for _ in 0..MAX_PAGES_PER_TICK {
            let mut params = json!({
                "filters": filters,
                "pagination": { "limit": PAGE_LIMIT, "cursor": cursor },
            });
            // The RPC rejects startLedger alongside a cursor.
            match &cursor {
                Some(_) => {}
                None => {
                    params["startLedger"] = json!(start);
                    params["pagination"] = json!({ "limit": PAGE_LIMIT });
                }
            }
            let result = self.rpc(&url, "getEvents", params).await?;
            let events: Vec<Value> = result["events"].as_array().cloned().unwrap_or_default();

            for raw in &events {
                if let Some(event) = parse_event(raw, &watch.network, &watch.contract_id) {
                    max_ledger_seen = max_ledger_seen.max(event.ledger);
                    self.repo.upsert_event(&event).await?;
                    // Failed calls moved nothing, so only successful events count.
                    if event.in_successful_contract_call {
                        if let Some(movement) = decode_transfer(&event.topics, event.value.as_deref()) {
                            let transfer = StellarTokenTransfer {
                                id: event.id.clone(),
                                network: event.network.clone(),
                                token_contract: event.contract_id.clone(),
                                kind: movement.kind.to_string(),
                                from: movement.from,
                                to: movement.to,
                                amount: movement.amount.to_string(),
                                ledger: event.ledger,
                            };
                            self.repo.upsert_transfer(&transfer).await?;
                        }
                    }
                }
            }

            let next = result["cursor"].as_str().map(str::to_string);
            if events.len() < PAGE_LIMIT as usize || next.is_none() {
                finished = true;
                break;
            }
            cursor = next;
        }

        if finished {
            // Whole retained window (or tail) read through the snapshot ledger.
            self.repo
                .set_watch_progress(&format!("{}:{}", watch.network, watch.contract_id), latest, true)
                .await?;
        } else if max_ledger_seen >= start {
            // Page cap hit mid-ledger: stop one ledger short so the partial
            // ledger is re-read next tick. Upserts make the re-read harmless.
            self.repo
                .set_watch_progress(
                    &format!("{}:{}", watch.network, watch.contract_id),
                    (max_ledger_seen - 1).max(watch.last_ledger),
                    false,
                )
                .await?;
        }
        Ok(())
    }

    async fn rpc(&self, url: &str, method: &str, params: Value) -> Result<Value, AppError> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response = self
            .http
            .post(url)
            .timeout(RPC_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("Soroban RPC unreachable: {e}")))?;
        if !response.status().is_success() {
            return Err(AppError::ExternalService(format!(
                "Soroban RPC {method} failed with status {}",
                response.status()
            )));
        }
        let data: Value = response
            .json()
            .await
            .map_err(|e| AppError::ExternalService(format!("Soroban RPC returned invalid JSON: {e}")))?;
        if let Some(message) = data["error"]["message"].as_str() {
            return Err(AppError::ExternalService(format!("Soroban RPC {method}: {message}")));
        }
        Ok(data["result"].clone())
    }
}

/// Maps one getEvents entry to the stored shape. Returns None for entries
/// missing the fields that identify them (id, transaction hash, ledger).
fn parse_event(raw: &Value, network: &str, contract_id: &str) -> Option<StellarContractEvent> {
    let rpc_id = raw["id"].as_str()?;
    let tx_hash = raw["txHash"].as_str()?.to_string();
    let ledger = raw["ledger"].as_u64()? as i64;
    let ledger_closed_at: Option<DateTime<Utc>> = raw["ledgerClosedAt"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let topics: Vec<String> = raw["topic"]
        .as_array()
        .map(|items| items.iter().filter_map(xdr_of).collect())
        .unwrap_or_default();

    Some(StellarContractEvent {
        id: format!("{network}:{rpc_id}"),
        network: network.to_string(),
        contract_id: contract_id.to_string(),
        ledger,
        ledger_closed_at,
        tx_hash,
        event_type: raw["type"].as_str().unwrap_or("contract").to_string(),
        topics,
        value: xdr_of(&raw["value"]),
        in_successful_contract_call: raw["inSuccessfulContractCall"].as_bool().unwrap_or(true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rpc_event_with_object_value_and_topics() {
        let raw = json!({
            "id": "0001-1",
            "type": "contract",
            "ledger": 123,
            "ledgerClosedAt": "2026-11-20T10:00:00Z",
            "contractId": "CABC",
            "txHash": "deadbeef",
            "topic": [{ "xdr": "AAAADw==" }, "AAAADw=="],
            "value": { "xdr": "AAAAAQ==" },
            "inSuccessfulContractCall": false
        });
        let event = parse_event(&raw, "testnet", "CABC").expect("parses");
        assert_eq!(event.id, "testnet:0001-1");
        assert_eq!(event.ledger, 123);
        assert_eq!(event.tx_hash, "deadbeef");
        assert_eq!(event.topics, vec!["AAAADw==".to_string(), "AAAADw==".to_string()]);
        assert_eq!(event.value.as_deref(), Some("AAAAAQ=="));
        assert!(!event.in_successful_contract_call);
        assert!(event.ledger_closed_at.is_some());
    }

    #[test]
    fn drops_event_without_transaction_hash() {
        let raw = json!({ "id": "x", "ledger": 1 });
        assert!(parse_event(&raw, "testnet", "CABC").is_none());
    }
}
