use mongodb::bson::oid::ObjectId;
use rand::RngCore;

use crate::model::api_key::{ApiKey, API_KEY_SCOPES};
use crate::repositories::api_key_repository::ApiKeyRepository;
use crate::utils::api_key_auth::{hash_api_key, API_KEY_PREFIX};
use crate::utils::error::AppError;

#[derive(Clone)]
pub struct ApiKeyService {
    repo: ApiKeyRepository,
}

impl ApiKeyService {
    pub fn new(repo: ApiKeyRepository) -> Self {
        Self { repo }
    }

    /// Generates a new raw key, stores only its hash, and returns the raw
    /// value once — the caller (handler) must surface it to the user
    /// immediately, since it can never be retrieved again.
    pub async fn create(
        &self,
        user_id: ObjectId,
        label: String,
        scopes: Vec<String>,
    ) -> Result<(ApiKey, String), AppError> {
        let unknown: Vec<&String> = scopes.iter().filter(|s| !API_KEY_SCOPES.contains(&s.as_str())).collect();
        if !unknown.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Unknown scope(s): {}",
                unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }

        let mut random_bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut random_bytes);
        let raw_key = format!("{API_KEY_PREFIX}{}", hex::encode(random_bytes));
        let key_hash = hash_api_key(&raw_key);
        let key_prefix = raw_key.chars().take(API_KEY_PREFIX.len() + 4).collect::<String>();

        let key = ApiKey::new(user_id, label, key_hash, key_prefix, scopes);
        let created = self.repo.insert(&key).await?;
        Ok((created, raw_key))
    }

    pub async fn list(&self, user_id: ObjectId) -> Result<Vec<ApiKey>, AppError> {
        self.repo.find_by_user(user_id).await
    }

    pub async fn revoke(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.repo.revoke(id, user_id).await
    }
}
