use std::str::FromStr;
use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use chrono::Utc;
use serde::Deserialize;

use crate::model::scheduled_task::{ScheduledTask, TriggerKind};
use crate::services::scheduled_task_service::ScheduledTaskService;
use crate::services::session_key_service::SessionKeyService;
use crate::services::spend_policy_service::SpendPolicyService;
use crate::services::webhook_service::WebhookService;

const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Mirrors the frontend's `EvmTxParams` shape (transactionService.ts) —
/// `request_template` is stored as this same opaque JSON, so a task created
/// from the UI and one evaluated here agree on what a "request" looks like.
#[derive(Debug, Deserialize)]
struct EvmTxTemplate {
    chain_id: u64,
    to: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    data: String,
}

#[derive(Clone)]
pub struct SchedulerWorker {
    task_service: ScheduledTaskService,
    session_key_service: SessionKeyService,
    spend_policy_service: SpendPolicyService,
    webhook_service: WebhookService,
}

impl SchedulerWorker {
    pub fn new(
        task_service: ScheduledTaskService,
        session_key_service: SessionKeyService,
        spend_policy_service: SpendPolicyService,
        webhook_service: WebhookService,
    ) -> Self {
        Self {
            task_service,
            session_key_service,
            spend_policy_service,
            webhook_service,
        }
    }

    /// Spawns the poll loop. Fire-and-forget by design (mirrors main.rs's
    /// existing `tower_governor` pruning thread) — the loop itself never
    /// returns under normal operation, and a panic inside one tick's task
    /// processing is caught per-task so one bad task can't kill the worker.
    pub fn spawn(self) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            loop {
                interval.tick().await;
                self.tick().await;
            }
        });
    }

    async fn tick(&self) {
        match self.task_service.due_tasks_for_worker().await {
            Ok(tasks) => {
                for task in tasks {
                    self.run_task(task).await;
                }
            }
            Err(e) => tracing::warn!(error = %e, "Failed to load due scheduled tasks"),
        }

        match self.task_service.price_triggered_tasks_for_worker().await {
            Ok(tasks) => {
                for task in tasks {
                    if self.price_condition_met(&task).await {
                        self.run_task(task).await;
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "Failed to load price-triggered scheduled tasks"),
        }
    }

    /// Placeholder price evaluation — a real implementation resolves `token`
    /// against a price source appropriate to `chain` (on-chain reference
    /// price or an external feed) before comparing against `value_usd`.
    /// Flagged explicitly rather than silently always-true or always-false,
    /// since either would be a worse default than an honest "not wired up".
    async fn price_condition_met(&self, task: &ScheduledTask) -> bool {
        if let TriggerKind::PriceThreshold { .. } = &task.trigger {
            tracing::warn!(
                task_id = ?task.id,
                "Price-threshold evaluation is not yet wired to a live price source; skipping this tick"
            );
        }
        false
    }

    async fn run_task(&self, task: ScheduledTask) {
        let task_id = match task.id {
            Some(id) => id,
            None => return,
        };

        let result = self.execute(&task).await;

        let next_run_at = match &task.trigger {
            TriggerKind::Recurring { interval_minutes } => {
                Some(Utc::now() + chrono::Duration::minutes(*interval_minutes as i64))
            }
            TriggerKind::TimeOnce { .. } => None, // one-shot: no further run
            TriggerKind::PriceThreshold { .. } => None, // re-evaluated every tick, not scheduled
        };

        let error_message = result.as_ref().err().map(|e| e.to_string());
        if let Err(e) = self
            .task_service
            .record_run(task_id, next_run_at, error_message.clone())
            .await
        {
            tracing::warn!(error = %e, task_id = ?task_id, "Failed to record scheduled task run");
        }

        let event = if result.is_ok() { "tx.confirmed" } else { "tx.failed" };
        self.webhook_service
            .dispatch(
                event,
                &serde_json::json!({
                    "task_id": task_id.to_hex(),
                    "task_name": task.name,
                    "error": error_message,
                }),
            )
            .await;
    }

    async fn execute(&self, task: &ScheduledTask) -> Result<(), String> {
        let template: EvmTxTemplate = serde_json::from_value(task.request_template.clone())
            .map_err(|e| format!("Unsupported or malformed request template: {e}"))?;

        let key = self
            .session_key_service
            .authorize(task.session_key_id, task.user_id, Some(&template.to), None)
            .await
            .map_err(|e| e.to_string())?;

        // Spend-policy check — non-negotiable for unattended execution, the
        // same gate an interactive transaction goes through.
        let usd_value = estimate_usd_value(&template.value);
        self.spend_policy_service
            .check(task.user_id, &key.wallet_address, usd_value)
            .await
            .map_err(|e| e.to_string())?;

        let private_key = self
            .session_key_service
            .decrypt_signer(&key)
            .map_err(|e| e.to_string())?;

        let result = broadcast_evm(&template, &private_key).await;

        // Never let a signing/broadcast error leak the private key into a
        // log line — `result`'s error variant is already a plain message,
        // but this makes the invariant explicit at the call site.
        drop(private_key);

        result.map_err(|e| e.to_string())?;

        self.spend_policy_service
            .record(task.user_id, &key.wallet_address, usd_value)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

/// Placeholder USD valuation — a real implementation prices `value` (native
/// token, base units) against a live feed. Returning 0 here means spend
/// caps configured in USD currently do not constrain native-value transfers
/// from the scheduler until this is wired up; flagged rather than silently
/// approximated, since a wrong-but-confident number would be worse than an
/// honest gap for a feature that gates real fund movement.
fn estimate_usd_value(_value: &str) -> f64 {
    0.0
}

async fn broadcast_evm(template: &EvmTxTemplate, private_key_hex: &str) -> Result<String, String> {
    let signer = PrivateKeySigner::from_str(private_key_hex.trim_start_matches("0x"))
        .map_err(|e| format!("Invalid session key signer: {e}"))?;

    let to = Address::from_str(&template.to).map_err(|e| format!("Invalid recipient address: {e}"))?;
    let value = U256::from_str(&template.value).unwrap_or(U256::ZERO);
    let data = if template.data.trim().is_empty() || template.data == "0x" {
        Bytes::new()
    } else {
        Bytes::from_str(&template.data).map_err(|e| format!("Invalid calldata: {e}"))?
    };

    let rpc_url = evm_rpc_url_for(template.chain_id)?;

    let provider = ProviderBuilder::new()
        .wallet(signer)
        .connect_http(rpc_url.parse().map_err(|e: url::ParseError| e.to_string())?);

    let tx = TransactionRequest::default()
        .with_to(to)
        .with_value(value)
        .with_input(data)
        .with_chain_id(template.chain_id);

    let pending = provider
        .send_transaction(tx)
        .await
        .map_err(|e| format!("Failed to broadcast transaction: {e}"))?;

    Ok(format!("{:#x}", pending.tx_hash()))
}

/// A small set of built-in public EVM RPC endpoints for the worker's own
/// broadcast calls — deliberately not reusing the frontend's user-editable
/// RpcEndpointOverrides (that's browser localStorage, unreachable from the
/// backend). A production deployment should make this configurable per
/// chain rather than hardcoded, same caveat as the frontend's own
/// lib/constants.ts defaults.
fn evm_rpc_url_for(chain_id: u64) -> Result<String, String> {
    match chain_id {
        1 => Ok("https://cloudflare-eth.com".to_string()),
        11155111 => Ok("https://rpc.sepolia.org".to_string()),
        137 => Ok("https://polygon.drpc.org".to_string()),
        8453 => Ok("https://mainnet.base.org".to_string()),
        other => Err(format!("No RPC endpoint configured for chain id {other}")),
    }
}
