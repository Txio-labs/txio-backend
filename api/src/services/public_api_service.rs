use mongodb::bson::oid::ObjectId;

use crate::dtos::public_api_dtos::ExecuteTransactionRequest;
use crate::repositories::idempotency_repository::{Begin, IdempotencyRepository};
use crate::model::history::HistoryEntry;
use crate::repositories::history_repository::HistoryRepository;
use crate::services::session_key_service::SessionKeyService;
use crate::services::spend_policy_service::SpendPolicyService;
use crate::utils::error::AppError;

/// EVM-only transaction template — same shape the scheduler worker already
/// consumes (services/scheduler_worker.rs's EvmTxTemplate), reused here
/// rather than re-deriving the broadcast logic for a second call path.
#[derive(Debug, serde::Deserialize)]
struct EvmTxTemplate {
    chain_id: u64,
    to: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    data: String,
}

#[derive(Clone)]
pub struct PublicApiService {
    history_repo: HistoryRepository,
    idempotency_repo: IdempotencyRepository,
    session_key_service: SessionKeyService,
    spend_policy_service: SpendPolicyService,
}

impl PublicApiService {
    pub fn new(
        history_repo: HistoryRepository,
        idempotency_repo: IdempotencyRepository,
        session_key_service: SessionKeyService,
        spend_policy_service: SpendPolicyService,
    ) -> Self {
        Self {
            history_repo,
            idempotency_repo,
            session_key_service,
            spend_policy_service,
        }
    }

    pub async fn history(
        &self,
        user_id: ObjectId,
        wallet_address: Option<String>,
        wallet_family: Option<String>,
        chain: Option<String>,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        if wallet_address.is_some() || wallet_family.is_some() || chain.is_some() {
            return self
                .history_repo
                .find_by_user_and_wallet(user_id, None, wallet_address, wallet_family, chain)
                .await;
        }
        self.history_repo.find_by_user(user_id, None).await
    }

    /// Server-side simulation does not exist yet: simulation runs in each chain
    /// adapter in the app, against that chain's own dry-run method. Answering
    /// with a made-up result would be worse than saying so, so this is an
    /// explicit 501 (`-32015`), not a 200 that looks like a check passed.
    pub fn simulate(&self, chain: &str) -> Result<(), AppError> {
        Err(AppError::NotImplemented(format!(
            "Server-side simulation is not available for {chain}. Simulate with the txio app or CLI, then call execute."
        )))
    }

    /// `execute` with an optional `Idempotency-Key`: a retried request with the
    /// same key and body returns the first result instead of broadcasting again.
    pub async fn execute_idempotent(
        &self,
        user_id: ObjectId,
        idempotency_key: Option<&str>,
        req: ExecuteTransactionRequest,
    ) -> Result<String, AppError> {
        let Some(key) = idempotency_key else {
            return self.execute(user_id, req).await;
        };

        let request_hash = {
            use sha2::{Digest, Sha256};
            let canonical = format!("{}|{}|{}", req.chain, req.session_key_id, req.tx_params);
            hex::encode(Sha256::digest(canonical.as_bytes()))
        };

        match self.idempotency_repo.begin(user_id, key, &request_hash).await? {
            Begin::Replay(hash) => Ok(hash),
            Begin::InProgress => Err(AppError::Conflict("A request with this Idempotency-Key is still in progress".into())),
            Begin::Mismatch => Err(AppError::Conflict("This Idempotency-Key was already used for a different request".into())),
            Begin::New => match self.execute(user_id, req).await {
                Ok(hash) => {
                    self.idempotency_repo.complete(user_id, key, &hash).await?;
                    Ok(hash)
                }
                Err(e) => {
                    let _ = self.idempotency_repo.abort(user_id, key).await;
                    Err(e)
                }
            },
        }
    }

    /// EVM-only for now (mirrors the scheduler worker's own scope) — signs
    /// and broadcasts via the caller's session key, gated by the same
    /// spend-policy check every other execution path in this app goes
    /// through, interactive or automated.
    pub async fn execute(&self, user_id: ObjectId, req: ExecuteTransactionRequest) -> Result<String, AppError> {
        if req.chain != "evm" {
            return Err(AppError::BadRequest(
                "Only EVM execution is supported via the public API today.".into(),
            ));
        }

        let session_key_id = ObjectId::parse_str(&req.session_key_id)
            .map_err(|_| AppError::BadRequest("Invalid session_key_id".into()))?;

        let template: EvmTxTemplate = serde_json::from_value(req.tx_params.clone())
            .map_err(|e| AppError::BadRequest(format!("Malformed tx_params for EVM: {e}")))?;

        let key = self
            .session_key_service
            .authorize(session_key_id, user_id, Some(&template.to), None)
            .await?;

        self.spend_policy_service.check(user_id, &key.wallet_address, 0.0).await?;

        let private_key = self.session_key_service.decrypt_signer(&key)?;
        let result = broadcast(&template, &private_key).await;
        drop(private_key);

        let hash = result.map_err(AppError::BadRequest)?;

        self.spend_policy_service.record(user_id, &key.wallet_address, 0.0).await?;

        Ok(hash)
    }
}

/// Thin re-implementation of scheduler_worker::broadcast_evm's signing step
/// — kept private and separate rather than exported cross-module, since the
/// two call sites (scheduled automation vs. a direct public-API call) have
/// different enough surrounding context that sharing one function would
/// couple them unnecessarily; the underlying alloy calls are identical.
async fn broadcast(template: &EvmTxTemplate, private_key_hex: &str) -> Result<String, String> {
    use alloy::network::TransactionBuilder;
    use alloy::primitives::{Address, Bytes, U256};
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::rpc::types::TransactionRequest;
    use alloy::signers::local::PrivateKeySigner;
    use std::str::FromStr;

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

fn evm_rpc_url_for(chain_id: u64) -> Result<String, String> {
    match chain_id {
        1 => Ok("https://cloudflare-eth.com".to_string()),
        11155111 => Ok("https://rpc.sepolia.org".to_string()),
        137 => Ok("https://polygon.drpc.org".to_string()),
        8453 => Ok("https://mainnet.base.org".to_string()),
        other => Err(format!("No RPC endpoint configured for chain id {other}")),
    }
}
