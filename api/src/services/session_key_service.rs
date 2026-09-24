use crate::dtos::session_key_dtos::CreateSessionKeyRequest;
use crate::model::session_key::SessionKey;
use crate::repositories::session_key_repository::SessionKeyRepository;
use crate::utils::error::AppError;
use crate::utils::session_key_crypto;
use mongodb::bson::oid::ObjectId;

#[derive(Clone)]
pub struct SessionKeyService {
    repo: SessionKeyRepository,
    encryption_key: String,
}

impl SessionKeyService {
    pub fn new(repo: SessionKeyRepository, encryption_key: String) -> Self {
        Self {
            repo,
            encryption_key,
        }
    }

    pub async fn create(
        &self,
        user_id: ObjectId,
        req: CreateSessionKeyRequest,
    ) -> Result<SessionKey, AppError> {
        if req.expires_at <= chrono::Utc::now() {
            return Err(AppError::BadRequest(
                "Expiry must be in the future".into(),
            ));
        }

        let encrypted_private_key =
            session_key_crypto::encrypt(&self.encryption_key, &req.delegate_private_key)?;

        let key = SessionKey::new(
            user_id,
            req.wallet_family,
            req.wallet_address,
            req.label,
            req.delegate_address,
            encrypted_private_key,
            req.scoped_contracts,
            req.max_amount_per_tx_usd,
            req.expires_at,
        );
        self.repo.insert(&key).await
    }

    pub async fn list(&self, user_id: ObjectId) -> Result<Vec<SessionKey>, AppError> {
        self.repo.find_by_user(user_id).await
    }

    pub async fn revoke(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.repo.revoke(id, user_id).await
    }

    /// Loaded by the scheduler worker before every automated execution —
    /// confirms the key is still active (not expired/revoked) and, when
    /// scoped, that the target contract is allowed.
    pub async fn authorize(
        &self,
        id: ObjectId,
        user_id: ObjectId,
        target_contract: Option<&str>,
        usd_value: Option<f64>,
    ) -> Result<SessionKey, AppError> {
        let key = self.repo.find_by_id(id, user_id).await?;

        if !key.is_active() {
            return Err(AppError::Forbidden(
                "This session key has expired or been revoked".into(),
            ));
        }

        if !key.scoped_contracts.is_empty() {
            match target_contract {
                Some(contract) if key.scoped_contracts.iter().any(|c| c.eq_ignore_ascii_case(contract)) => {}
                _ => {
                    return Err(AppError::Forbidden(
                        "This session key is not scoped to the target contract".into(),
                    ));
                }
            }
        }

        if let (Some(max), Some(value)) = (key.max_amount_per_tx_usd, usd_value) {
            if value > max {
                return Err(AppError::Forbidden(format!(
                    "Transaction value ${value:.2} exceeds this session key's ${max:.2} per-transaction limit"
                )));
            }
        }

        Ok(key)
    }

    /// Decrypts an already-authorized key's ephemeral signer — call only
    /// after `authorize` has succeeded, immediately before signing, and
    /// never log or persist the returned plaintext.
    pub fn decrypt_signer(&self, key: &SessionKey) -> Result<String, AppError> {
        session_key_crypto::decrypt(&self.encryption_key, &key.encrypted_private_key)
    }
}
