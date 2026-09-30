use bcrypt::{hash, verify, DEFAULT_COST};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use mongodb::bson::oid::ObjectId;
use rand::Rng;
use rand::RngCore;
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::dtos::webhook_subscription_dtos::CreateWebhookRequest;
use crate::model::webhook_delivery::WebhookDelivery;
use crate::model::webhook_subscription::{WebhookSubscription, WEBHOOK_EVENTS};
use crate::repositories::webhook_delivery_repository::WebhookDeliveryRepository;
use crate::repositories::webhook_subscription_repository::WebhookSubscriptionRepository;
use crate::utils::error::AppError;
use crate::utils::session_key_crypto;
use crate::utils::url_safety::validate_https_url;

type HmacSha256 = Hmac<Sha256>;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// A claimed delivery that is not finished within this window becomes due
/// again (worker crash, restart).
const LEASE: ChronoDuration = ChronoDuration::seconds(60);
const IDLE_POLL: Duration = Duration::from_secs(3);
const MAX_CONCURRENT_DELIVERIES: usize = 8;
/// Seconds to wait after failed attempt 1, 2, 3, ...; a delivery is dead after the last one fails.
const BACKOFF_SECONDS: [i64; 6] = [10, 60, 300, 1800, 7200, 21600];
/// Receivers should reject signatures older than this (replay protection).
pub const SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

#[derive(Clone)]
pub struct WebhookService {
    repo: WebhookSubscriptionRepository,
    deliveries: WebhookDeliveryRepository,
    encryption_key: String,
    client: reqwest::Client,
}

/// Generates a random secret (shown once) and returns it alongside its bcrypt
/// hash (kept for the "is this the secret?" check) and its encrypted form
/// (kept so deliveries can be signed with the real secret).
fn generate_secret(encryption_key: &str) -> Result<(String, String, String), AppError> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = format!("whsec_{}", hex::encode(bytes));
    let secret_hash = hash(&secret, DEFAULT_COST)
        .map_err(|_| AppError::InternalError("Failed to hash webhook secret".into()))?;
    let secret_enc = session_key_crypto::encrypt(encryption_key, &secret)?;
    Ok((secret, secret_hash, secret_enc))
}

/// `v1=<hex HMAC-SHA256(secret, "<timestamp>.<body>")>`. Binding the timestamp
/// into the MAC lets receivers reject replays.
pub fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}

/// When to try again after `attempts` failed attempts, or `None` when the
/// delivery has used them all. Includes +-20% jitter so a recovering receiver
/// is not hit by every retry at once.
pub fn next_retry_delay(attempts: u32, jitter: f64) -> Option<ChronoDuration> {
    let base = *BACKOFF_SECONDS.get(attempts.checked_sub(1)? as usize)?;
    let factor = 1.0 + (jitter.clamp(0.0, 1.0) - 0.5) * 0.4;
    Some(ChronoDuration::milliseconds((base as f64 * 1000.0 * factor) as i64))
}

impl WebhookService {
    pub fn new(
        repo: WebhookSubscriptionRepository,
        deliveries: WebhookDeliveryRepository,
        encryption_key: String,
    ) -> Self {
        Self {
            repo,
            deliveries,
            encryption_key,
            client: reqwest::Client::builder()
                .timeout(DELIVERY_TIMEOUT)
                // A validated URL could still 3xx to an internal address.
                .redirect(reqwest::redirect::Policy::none())
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

        let (secret, secret_hash, secret_enc) = generate_secret(&self.encryption_key)?;
        let sub = WebhookSubscription::new(user_id, req.url, req.events, secret_hash, Some(secret_enc));
        let created = self.repo.insert(&sub).await?;
        Ok((created, secret))
    }

    pub async fn list(&self, user_id: ObjectId) -> Result<Vec<WebhookSubscription>, AppError> {
        self.repo.find_by_user(user_id).await
    }

    pub async fn delete(&self, id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.repo.delete(id, user_id).await
    }

    /// Issues a new signing secret, shown once. The old one stops signing
    /// immediately. This is also how a subscription created before secrets
    /// were recoverable starts producing verifiable signatures.
    pub async fn rotate_secret(&self, id: ObjectId, user_id: ObjectId) -> Result<String, AppError> {
        self.repo.find_owned(id, user_id).await?;
        let (secret, secret_hash, secret_enc) = generate_secret(&self.encryption_key)?;
        self.repo.replace_secret(id, user_id, secret_hash, secret_enc).await?;
        Ok(secret)
    }

    pub async fn recent_deliveries(
        &self,
        id: ObjectId,
        user_id: ObjectId,
    ) -> Result<Vec<WebhookDelivery>, AppError> {
        self.repo.find_owned(id, user_id).await?;
        self.deliveries.list_for_subscription(id, user_id, 50).await
    }

    pub async fn redeliver(&self, subscription_id: ObjectId, delivery_id: ObjectId, user_id: ObjectId) -> Result<(), AppError> {
        self.repo.find_owned(subscription_id, user_id).await?;
        self.deliveries.requeue(delivery_id, user_id).await
    }

    /// Queues an event for every active subscription listening for it. This
    /// only writes rows; the worker does the sending, so a slow or down
    /// receiver never blocks the caller and nothing is lost on restart.
    pub async fn dispatch(&self, event: &str, payload: &serde_json::Value) {
        let subs = match self.repo.find_active_for_event(event).await {
            Ok(subs) => subs,
            Err(e) => {
                tracing::warn!(error = %e, event, "Failed to load webhook subscriptions for event");
                return;
            }
        };

        for sub in subs {
            let Some(sub_id) = sub.id else { continue };
            let delivery_id = uuid::Uuid::new_v4().to_string();
            let body = serde_json::json!({
                "id": delivery_id,
                "event": event,
                "created_at": Utc::now().to_rfc3339(),
                "data": payload,
            });
            let Ok(body) = serde_json::to_string(&body) else { continue };
            let delivery = WebhookDelivery::new(sub_id, sub.user_id, event.to_string(), delivery_id, body);
            if let Err(e) = self.deliveries.insert(&delivery).await {
                tracing::warn!(error = %e, event, "Failed to queue webhook delivery");
            }
        }
    }

    /// Background loop: claims due deliveries and sends them, up to
    /// `MAX_CONCURRENT_DELIVERIES` at a time.
    pub fn spawn_worker(&self) {
        let service = self.clone();
        tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_DELIVERIES));
            loop {
                let permit = match permits.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                match service.deliveries.claim_due(LEASE).await {
                    Ok(Some(delivery)) => {
                        let worker = service.clone();
                        tokio::spawn(async move {
                            worker.attempt(delivery).await;
                            drop(permit);
                        });
                    }
                    Ok(None) => {
                        drop(permit);
                        tokio::time::sleep(IDLE_POLL).await;
                    }
                    Err(e) => {
                        drop(permit);
                        tracing::warn!(error = %e, "Webhook delivery queue read failed");
                        tokio::time::sleep(IDLE_POLL).await;
                    }
                }
            }
        });
    }

    async fn attempt(&self, delivery: WebhookDelivery) {
        let Some(id) = delivery.id else { return };

        let sub = match self.repo.find_by_id(delivery.subscription_id).await {
            Ok(Some(sub)) if sub.is_active => sub,
            Ok(_) => {
                let _ = self.deliveries.mark_dead(id, None, "Subscription removed or disabled".into()).await;
                return;
            }
            Err(e) => {
                self.retry_or_die(&delivery, None, format!("Subscription lookup failed: {e}")).await;
                return;
            }
        };

        // The URL is checked again on every attempt: DNS can change between
        // creation and delivery.
        if let Err(e) = validate_https_url(&sub.url).await {
            let _ = self.deliveries.mark_dead(id, None, format!("Blocked URL: {e}")).await;
            let _ = self.repo.record_delivery(delivery.subscription_id, Some(format!("Blocked URL: {e}"))).await;
            return;
        }

        let secret = match sub.secret_enc.as_deref().map(|enc| session_key_crypto::decrypt(&self.encryption_key, enc)) {
            Some(Ok(secret)) => secret,
            _ => {
                // Signing with the stored hash gives a signature nobody can verify.
                let _ = self
                    .deliveries
                    .mark_dead(id, None, "Subscription has no recoverable secret; rotate it".into())
                    .await;
                let _ = self
                    .repo
                    .record_delivery(delivery.subscription_id, Some("No recoverable secret; rotate the secret".into()))
                    .await;
                return;
            }
        };

        let timestamp = Utc::now().timestamp();
        let signature = sign(&secret, timestamp, delivery.body.as_bytes());
        let response = self
            .client
            .post(&sub.url)
            .header("Content-Type", "application/json")
            .header("X-Txio-Event", &delivery.event)
            .header("X-Txio-Delivery", &delivery.delivery_id)
            .header("X-Txio-Timestamp", timestamp.to_string())
            .header("X-Txio-Signature", signature)
            .body(delivery.body.clone())
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let _ = self.deliveries.mark_delivered(id, resp.status().as_u16()).await;
                let _ = self.repo.record_delivery(delivery.subscription_id, None).await;
            }
            Ok(resp) if resp.status().as_u16() == 410 => {
                let _ = self.deliveries.mark_dead(id, Some(410), "Receiver answered 410 Gone".into()).await;
                let _ = self.repo.deactivate(delivery.subscription_id, "Receiver answered 410 Gone".into()).await;
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                self.retry_or_die(&delivery, Some(code), format!("HTTP {code}")).await;
            }
            Err(e) => self.retry_or_die(&delivery, None, e.to_string()).await,
        }
    }

    async fn retry_or_die(&self, delivery: &WebhookDelivery, status: Option<u16>, error: String) {
        let Some(id) = delivery.id else { return };
        let _ = self.repo.record_delivery(delivery.subscription_id, Some(error.clone())).await;
        // Drawn before the match so the thread-local RNG is not held across an await.
        let jitter: f64 = rand::rng().random();
        match next_retry_delay(delivery.attempts, jitter) {
            Some(delay) => {
                let at: DateTime<Utc> = Utc::now() + delay;
                let _ = self.deliveries.mark_retry(id, at, status, error).await;
            }
            None => {
                let _ = self.deliveries.mark_dead(id, status, error).await;
            }
        }
    }
}

/// Verifies a previously-shown secret against its stored hash.
#[allow(dead_code)]
pub fn verify_secret(secret: &str, secret_hash: &str) -> bool {
    verify(secret, secret_hash).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_bound_to_secret_timestamp_and_body() {
        let a = sign("whsec_a", 1000, b"{}");
        assert!(a.starts_with("v1=") && a.len() == 3 + 64);
        assert_eq!(a, sign("whsec_a", 1000, b"{}"));
        assert_ne!(a, sign("whsec_b", 1000, b"{}"));
        assert_ne!(a, sign("whsec_a", 1001, b"{}"));
        assert_ne!(a, sign("whsec_a", 1000, b"{ }"));
    }

    #[test]
    fn signature_matches_the_documented_construction() {
        // What a receiver computes: HMAC-SHA256(secret, "<ts>.<body>"), hex, "v1=" prefix.
        let mut mac = HmacSha256::new_from_slice(b"whsec_x").unwrap();
        mac.update(b"1700000000.{\"a\":1}");
        let expected = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
        assert_eq!(sign("whsec_x", 1_700_000_000, b"{\"a\":1}"), expected);
    }

    #[test]
    fn backoff_follows_the_schedule_and_then_gives_up() {
        let mid = |attempts| next_retry_delay(attempts, 0.5).map(|d| d.num_seconds());
        assert_eq!(mid(1), Some(10));
        assert_eq!(mid(2), Some(60));
        assert_eq!(mid(6), Some(21600));
        assert_eq!(mid(7), None);
        assert_eq!(mid(0), None);
    }

    #[test]
    fn jitter_stays_within_twenty_percent() {
        let low = next_retry_delay(3, 0.0).unwrap().num_seconds();
        let high = next_retry_delay(3, 1.0).unwrap().num_seconds();
        assert_eq!((low, high), (240, 360));
    }

    #[test]
    fn secrets_round_trip_through_encryption_and_hash() {
        let (secret, secret_hash, secret_enc) = generate_secret("a-long-enough-test-encryption-key-123").unwrap();
        assert!(secret.starts_with("whsec_"));
        assert!(verify_secret(&secret, &secret_hash));
        assert_eq!(session_key_crypto::decrypt("a-long-enough-test-encryption-key-123", &secret_enc).unwrap(), secret);
    }
}
