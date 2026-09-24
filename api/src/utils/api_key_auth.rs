use axum::{extract::FromRequestParts, http::request::Parts, Extension};
use mongodb::bson::oid::ObjectId;
use sha2::{Digest, Sha256};

use crate::repositories::api_key_repository::ApiKeyRepository;
use crate::utils::error::AppError;

pub const API_KEY_PREFIX: &str = "txio_live_";

/// Resolves a public-API request's `Authorization: Bearer txio_live_...`
/// header to the key's owning user — the public-API equivalent of `Claims`
/// (JWT auth), used the same way (`api_key: ApiKeyAuth` as a handler
/// parameter). The one structural difference from `Claims`: this extractor
/// needs database access to look up the hashed key, not just a static
/// secret, so it pulls a shared `ApiKeyRepository` via `Extension` instead
/// of `JwtHelper`.
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    pub user_id: ObjectId,
    pub api_key_id: ObjectId,
    pub scopes: Vec<String>,
}

impl ApiKeyAuth {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    pub fn require_scope(&self, scope: &str) -> Result<(), AppError> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(AppError::Forbidden(format!("This API key does not have the '{scope}' scope")))
        }
    }
}

pub fn hash_api_key(raw_key: &str) -> String {
    let digest = Sha256::digest(raw_key.as_bytes());
    hex::encode(digest)
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for ApiKeyAuth
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("Missing authorization header".to_string()))?;

        let raw_key = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("Invalid authorization header format".to_string()))?
            .to_string();

        if !raw_key.starts_with(API_KEY_PREFIX) {
            return Err(AppError::Unauthorized("Not a valid Txio API key".to_string()));
        }

        let Extension(repo) = Extension::<ApiKeyRepository>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::InternalError("API key repository not configured".into()))?;

        let key_hash = hash_api_key(&raw_key);
        let key = repo
            .find_by_hash(&key_hash)
            .await?
            .ok_or_else(|| AppError::Unauthorized("Invalid API key".to_string()))?;

        if !key.is_active() {
            return Err(AppError::Unauthorized("This API key has been revoked".to_string()));
        }

        let key_id = key.id.ok_or_else(|| AppError::InternalError("API key missing id".into()))?;

        // Fire-and-forget — never let a usage-stamp failure block a
        // legitimate request.
        let repo_for_touch = repo.clone();
        tokio::spawn(async move {
            let _ = repo_for_touch.touch_last_used(key_id).await;
        });

        Ok(ApiKeyAuth {
            user_id: key.user_id,
            api_key_id: key_id,
            scopes: key.scopes,
        })
    }
}
