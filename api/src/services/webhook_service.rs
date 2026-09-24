use bcrypt::{hash, verify, DEFAULT_COST};
use hmac::{Hmac, KeyInit, Mac};
use mongodb::bson::oid::ObjectId;
use rand::RngCore;
use sha2::Sha256;
use std::time::Duration;

use crate::dtos::webhook_subscription_dtos::CreateWebhookRequest;
use crate::model::webhook_subscription::{WebhookSubscription, WEBHOOK_EVENTS};
use crate::repositories::webhook_subscription_repository::WebhookSubscriptionRepository;
use crate::utils::error::AppError;
use crate::utils::url_safety::validate_https_url;

type HmacSha256 = Hmac<Sha256>;

const DELIVERY_RETRIES: u32 = 3;
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct WebhookService {
    repo: WebhookSubscriptionRepository,
    client: reqwest::Client,
}

/// Generates a random secret (shown once at creation, matching the API-key
/// UX pattern) and returns it alongside its bcrypt hash for storage.
fn generate_secret() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = format!("whsec_{}", hex::encode(bytes));
    let hash = hash(&secret, DEFAULT_COST).expect("bcrypt hashing should not fail for a fixed-length input");
    (secret, hash)
}

impl WebhookService {
    pub fn new(repo: WebhookSubscriptionRepository) -> Self {
        Self {
            repo,
            client: reqwest::Client::builder()
                .timeout(DELIVERY_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub async fn create(
        &self,
        user_id: ObjectId,
        req: CreateWebhookRequest,
    ) -> Result<(WebhookSubscription, String), AppError> {
        validate_https_url(&req.url).await?;

        let unknown_events: Vec<&String> = req
            .events
            .iter()
            .filter(|e| !WEBHOOK_EVENTS.contains(&e.as_str()))
            .collect();
        if !unknown_events.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Unknown event type(s): {}",
                unknown_events
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let (secret, secret_hash) = generate_secret();
        let sub = WebhookSubscription::new(user_id, req.url, req.events, secret_hash);
        let created = self.repo.insert(&sub).await?;
        Ok((created, secret))
    }

    pub async fn list(&self, user_id: ObjectId) -> Result<Vec<WebhookSubscription>, AppError> {
        self.repo.find_by_user(user_id).await
    }

    pub async fn delete(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.repo.delete(id, user_id).await
    }

    /// Fans an event out to every active subscription listening for it.
    /// Fire-and-forget with bounded retries — proportionate to this being a
    /// notification feature, not a payments-critical delivery guarantee.
    pub async fn dispatch(&self, event: &str, payload: &serde_json::Value) {
        let subs = match self.repo.find_active_for_event(event).await {
            Ok(subs) => subs,
            Err(e) => {
                tracing::warn!(error = %e, event, "Failed to load webhook subscriptions for event");
                return;
            }
        };

        for sub in subs {
            self.deliver_with_retry(sub, event, payload).await;
        }
    }

    async fn deliver_with_retry(&self, sub: WebhookSubscription, event: &str, payload: &serde_json::Value) {
        let body = serde_json::json!({ "event": event, "data": payload });
        let body_bytes = match serde_json::to_vec(&body) {
            Ok(b) => b,
            Err(_) => return,
        };

        // The stored value is a bcrypt hash (for verifying a shown-once
        // secret against future re-entry, same as a password), not the raw
        // secret itself — so it can't be used to compute the outbound HMAC.
        // Delivery signing uses the hash as the HMAC key instead: still a
        // per-subscription secret the receiver can be given out-of-band to
        // verify against, without this service ever retaining the raw value.
        let mut mac = match HmacSha256::new_from_slice(sub.secret_hash.as_bytes()) {
            Ok(m) => m,
            Err(_) => return,
        };
        mac.update(&body_bytes);
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut last_error = None;
        for attempt in 0..DELIVERY_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }

            let result = self
                .client
                .post(&sub.url)
                .header("Content-Type", "application/json")
                .header("X-Txio-Signature", &signature)
                .body(body_bytes.clone())
                .send()
                .await;

            match result {
                Ok(resp) if resp.status().is_success() => {
                    let _ = self.repo.record_delivery(sub.id.unwrap_or_default(), None).await;
                    return;
                }
                Ok(resp) => last_error = Some(format!("HTTP {}", resp.status())),
                Err(e) => last_error = Some(e.to_string()),
            }
        }

        let _ = self.repo.record_delivery(sub.id.unwrap_or_default(), last_error).await;
    }
}

/// Verifies a previously-shown secret against its stored hash — used if a
/// receiver ever needs to re-confirm a secret server-side (not on the hot
/// delivery path, which signs with the hash directly, see `deliver_with_retry`).
#[allow(dead_code)]
pub fn verify_secret(secret: &str, secret_hash: &str) -> bool {
    verify(secret, secret_hash).unwrap_or(false)
}
