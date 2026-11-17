use crate::model::desktop_auth_code::DesktopAuthCode;
use crate::utils::error::AppError;
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Collection, Database, IndexModel};
use std::time::Duration as StdDuration;

/// How long a desktop sign-in code can be redeemed. MongoDB's TTL monitor runs
/// about once a minute, so redemption also checks `created_at` itself.
pub const DESKTOP_CODE_TTL_SECONDS: u64 = 300;

#[derive(Clone)]
pub struct DesktopAuthCodeRepository {
    collection: Collection<DesktopAuthCode>,
}

impl DesktopAuthCodeRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            collection: db.collection("desktop_auth_codes"),
        }
    }

    pub async fn ensure_indexes(&self) -> Result<(), AppError> {
        let ttl_index = IndexModel::builder()
            .keys(doc! { "created_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("desktop_auth_codes_created_at_ttl".to_string()))
                    .expire_after(StdDuration::from_secs(DESKTOP_CODE_TTL_SECONDS))
                    .build(),
            )
            .build();
        let code_index = IndexModel::builder()
            .keys(doc! { "code_hash": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("desktop_auth_codes_code_hash_unique".to_string()))
                    .unique(true)
                    .build(),
            )
            .build();
        self.collection
            .create_indexes(vec![ttl_index, code_index], None)
            .await
            .map(|_| ())
            .map_err(AppError::Database)
    }

    pub async fn save(&self, code: &DesktopAuthCode) -> Result<(), AppError> {
        self.collection.insert_one(code, None).await?;
        Ok(())
    }

    /// Remove and return the row for a code hash. Atomic, so a code can be
    /// redeemed at most once even under concurrent requests.
    pub async fn take(&self, code_hash: &str) -> Result<Option<DesktopAuthCode>, AppError> {
        Ok(self
            .collection
            .find_one_and_delete(doc! { "code_hash": code_hash }, None)
            .await?)
    }
}
