use crate::dtos::spend_policy_dtos::UpsertSpendPolicyRequest;
use crate::model::spend_policy::SpendPolicy;
use crate::repositories::spend_policy_repository::SpendPolicyRepository;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;

#[derive(Clone)]
pub struct SpendPolicyService {
    repo: SpendPolicyRepository,
}

impl SpendPolicyService {
    pub fn new(repo: SpendPolicyRepository) -> Self {
        Self { repo }
    }

    pub async fn upsert(
        &self,
        user_id: ObjectId,
        req: UpsertSpendPolicyRequest,
    ) -> Result<SpendPolicy, AppError> {
        let policy = SpendPolicy::new(
            user_id,
            req.wallet_family,
            req.wallet_address,
            req.chain,
            req.daily_limit_usd,
            req.per_tx_limit_usd,
            req.max_tx_per_hour,
        );
        self.repo.upsert(&policy).await
    }

    pub async fn get(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
    ) -> Result<Option<SpendPolicy>, AppError> {
        self.repo.find(user_id, wallet_address).await
    }

    pub async fn delete(&self, user_id: ObjectId, wallet_address: &str) -> Result<(), AppError> {
        self.repo.delete(user_id, wallet_address).await
    }

    /// The actual enforcement gate — called before any transaction executes,
    /// interactive or automated. Returns `Ok(())` when the transaction may
    /// proceed, or a `BadRequest`/`Forbidden` naming the specific limit hit.
    /// A wallet with no configured policy has no caps (opt-in, not a silent
    /// default limit that would surprise an existing user).
    pub async fn check(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
        usd_value: f64,
    ) -> Result<(), AppError> {
        let policy = match self.repo.find(user_id, wallet_address).await? {
            Some(p) => p,
            None => return Ok(()),
        };

        if let Some(per_tx) = policy.per_tx_limit_usd {
            if usd_value > per_tx {
                return Err(AppError::Forbidden(format!(
                    "Transaction value ${usd_value:.2} exceeds this wallet's per-transaction limit of ${per_tx:.2}"
                )));
            }
        }

        let usage = self.repo.today_usage(user_id, wallet_address).await?;

        if let Some(daily) = policy.daily_limit_usd {
            if usage.spent_usd + usd_value > daily {
                return Err(AppError::Forbidden(format!(
                    "This transaction would bring today's spend to ${:.2}, over the ${daily:.2} daily limit",
                    usage.spent_usd + usd_value
                )));
            }
        }

        if let Some(max_per_hour) = policy.max_tx_per_hour {
            // tx_count is a whole-day bucket, not a rolling hour — a coarser
            // but simpler v1 approximation (see model docs); tightened to a
            // real rolling window if this proves too permissive in practice.
            if usage.tx_count >= max_per_hour {
                return Err(AppError::Forbidden(format!(
                    "This wallet has reached its limit of {max_per_hour} transactions for the current window"
                )));
            }
        }

        Ok(())
    }

    /// Records a completed transaction's value against today's usage bucket
    /// — called after successful execution, never before (a blocked
    /// transaction shouldn't count against the limit that blocked it).
    pub async fn record(
        &self,
        user_id: ObjectId,
        wallet_address: &str,
        usd_value: f64,
    ) -> Result<(), AppError> {
        self.repo.record_usage(user_id, wallet_address, usd_value).await
    }
}
