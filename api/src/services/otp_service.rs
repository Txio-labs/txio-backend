use crate::model::otp::OTP;
use crate::repositories::otp_repository::OTPRepository;
use crate::utils::error::AppError;
use crate::utils::generate_otp::generate_otp;
use chrono::{Duration, Utc};

const OTP_LENGTH: usize = 6;
pub(crate) const OTP_VALIDITY_MINUTES: i64 = 5;
const OTP_SEND_COOLDOWN_SECONDS: i64 = 60;
const OTP_MAX_FAILED_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct OTPService {
    repository: OTPRepository,
}

impl OTPService {
    pub fn new(repository: OTPRepository) -> Self {
        OTPService { repository }
    }

    pub async fn generate_otp(&self, email: &str) -> Result<String, AppError> {
        let now = Utc::now();

        // Check cooldown regardless of row state (locked or active). This
        // prevents a resend-cooldown bypass where the attacker forces row
        // deletion via cap-out or expiry, clearing the cooldown check.
        if let Ok(existing_otp) = self.repository.find_by_email(email).await {
            if now < existing_otp.created_at + Duration::seconds(OTP_SEND_COOLDOWN_SECONDS) {
                return Err(AppError::BadRequest(
                    "OTP request rate limit exceeded. Please try again later.".into(),
                ));
            }

            // Cooldown has passed — remove the old row (locked or expired) before issuing a new one.
            let _ = self.repository.delete_by_email(email).await;
        }

        let code = generate_otp(OTP_LENGTH);
        let otp = OTP::new(email.to_string(), code.clone());
        self.repository
            .upsert_otp(&otp, OTP_SEND_COOLDOWN_SECONDS)
            .await?;

        Ok(code)
    }

    pub async fn verify_otp(&self, email: &str, code: &str) -> Result<bool, AppError> {
        let otp = match self.repository.find_by_email(email).await {
            Ok(otp) => otp,
            Err(AppError::NotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        };

        // Reject locked rows immediately — the cap was already hit.
        if otp.locked {
            return Ok(false);
        }

        let now = Utc::now();
        if now > otp.created_at + Duration::minutes(OTP_VALIDITY_MINUTES) {
            // Expired: delete so the next generate_otp call can proceed after
            // the cooldown window elapses from `created_at`.
            let _ = self.repository.delete_by_email(email).await;
            return Ok(false);
        }

        if !constant_time_eq(&otp.otp, code) {
            // Atomically increment the counter and derive the cap decision from
            // the post-increment value returned by the atomic operation.
            // This eliminates the lost-update race when concurrent wrong-code
            // guesses all read the same stale `failed_attempts` baseline.
            match self.repository.inc_failed_attempts(email).await {
                Ok(updated) if updated.failed_attempts >= OTP_MAX_FAILED_ATTEMPTS => {
                    // Lock the row in place instead of deleting it; `generate_otp`
                    // will still find `created_at` and enforce the cooldown.
                    let _ = self.repository.lock_row(email).await;
                }
                _ => {}
            }
            return Ok(false);
        }

        self.repository.delete_by_email(email).await?;
        Ok(true)
    }
}

/// Compares two strings in constant time relative to their length, so that
/// early-exit timing cannot be used to learn how much of a secret matched.
/// Shared by OTP verification and OAuth CSRF-state checks.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }

    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_matching() {
        assert!(constant_time_eq("123456", "123456"));
    }

    #[test]
    fn test_constant_time_eq_mismatch() {
        assert!(!constant_time_eq("123456", "654321"));
        assert!(!constant_time_eq("123456", "12345"));
    }
}
