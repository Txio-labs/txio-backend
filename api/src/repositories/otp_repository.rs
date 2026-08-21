use crate::model::otp::OTP;
use crate::utils::error::AppError;
use chrono::Duration as ChronoDuration;
use mongodb::bson::doc;
use mongodb::options::{FindOneAndUpdateOptions, IndexOptions, ReturnDocument};
use mongodb::{Collection, Database, IndexModel};
use std::time::Duration as StdDuration;

/// MongoDB TTL for OTP documents — purges rows even if the app never verifies them.
const OTP_TTL_SECONDS: u64 = 600;

#[derive(Clone)]
pub struct OTPRepository {
    collection: Collection<OTP>,
}

impl OTPRepository {
    pub fn new(db: &Database) -> Self {
        let collection = db.collection("otps");
        Self { collection }
    }

    pub async fn ensure_indexes(&self) -> Result<(), AppError> {
        let ttl_index = IndexModel::builder()
            .keys(doc! { "created_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("otps_created_at_ttl".to_string()))
                    .expire_after(StdDuration::from_secs(OTP_TTL_SECONDS))
                    .build(),
            )
            .build();

        let email_index = IndexModel::builder()
            .keys(doc! { "email": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("otps_email_idx".to_string()))
                    .build(),
            )
            .build();

        self.collection
            .create_indexes(vec![ttl_index, email_index], None)
            .await
            .map(|_| ())
            .map_err(AppError::Database)
    }

    pub async fn save(&self, otp: &OTP) -> Result<OTP, AppError> {
        let result = self.collection.insert_one(otp, None).await?;

        let mut otp_with_id = otp.clone();
        if let Some(inserted_id) = result.inserted_id.as_object_id() {
            otp_with_id.id = Some(inserted_id);
        }

        Ok(otp_with_id)
    }

    pub async fn upsert_otp(&self, otp: &OTP, cooldown_seconds: i64) -> Result<OTP, AppError> {
        let cooldown_cutoff = otp.created_at - ChronoDuration::seconds(cooldown_seconds);

        let filter = doc! {
            "email": &otp.email,
            "created_at": { "$lte": mongodb::bson::DateTime::from_millis(cooldown_cutoff.timestamp_millis()) }
        };

        let options = FindOneAndReplaceOptions::builder().upsert(true).build();

        match self
            .collection
            .find_one_and_replace(filter, otp, options)
            .await
        {
            Ok(_) => Ok(otp.clone()),
            Err(e) => {
                if is_duplicate_key_error(&e) {
                    Err(AppError::BadRequest(
                        "OTP request rate limit exceeded. Please try again later.".to_string(),
                    ))
                } else {
                    Err(AppError::Database(e))
                }
            }
        }
    }

    pub async fn find_by_email(&self, email: &str) -> Result<OTP, AppError> {
        let otp = self
            .collection
            .find_one(doc! { "email": email }, None)
            .await?
            .ok_or(AppError::NotFound("OTP not found for email".to_string()))?;

        Ok(otp)
    }

    /// Atomically increments `failed_attempts` by 1 using `$inc` and returns
    /// the post-increment document. Unlike a read-then-write with `$set`, this
    /// is safe under concurrent requests — each guess lands exactly once.
    pub async fn inc_failed_attempts(&self, email: &str) -> Result<OTP, AppError> {
        let opts = FindOneAndUpdateOptions::builder()
            .return_document(ReturnDocument::After)
            .build();

        let updated = self
            .collection
            .find_one_and_update(
                doc! { "email": email },
                doc! { "$inc": { "failed_attempts": 1 } },
                opts,
            )
            .await?
            .ok_or(AppError::NotFound("OTP not found for email".to_string()))?;

        Ok(updated)
    }

    /// Marks the OTP row as locked (cap-out) in place. The row is NOT
    /// deleted; it remains until the TTL index removes it so that
    /// `generate_otp` can still read `created_at` and enforce the resend
    /// cooldown even after a failed-attempt cap-out.
    pub async fn lock_row(&self, email: &str) -> Result<(), AppError> {
        self.collection
            .update_one(
                doc! { "email": email },
                doc! { "$set": { "locked": true } },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn delete_by_email(&self, email: &str) -> Result<(), AppError> {
        self.collection
            .delete_many(doc! { "email": email }, None)
            .await?;
        Ok(())
    }
}

fn is_duplicate_key_error(err: &mongodb::error::Error) -> bool {
    if let mongodb::error::ErrorKind::Command(ref cmd_err) = *err.kind {
        if cmd_err.code == 11000 || cmd_err.code == 11001 || cmd_err.code == 12582 {
            return true;
        }
    }
    if let mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(
        ref write_err,
    )) = *err.kind
    {
        if write_err.code == 11000 || write_err.code == 11001 || write_err.code == 12582 {
            return true;
        }
    }
    let err_str = err.to_string();
    err_str.contains("E11000")
        || err_str.contains("duplicate key")
        || err_str.contains("DuplicateKey")
}
