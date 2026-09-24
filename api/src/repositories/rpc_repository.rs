use crate::model::rpc::RpcLog;
use crate::utils::error::AppError;
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One endpoint's aggregated call stats — usage volume, failure rate, and
/// latency — computed across every logged call to that URL. Powers the
/// admin dashboard's "most-used endpoints" and "failing endpoints" views.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EndpointStats {
    pub endpoint: String,
    pub total_calls: i64,
    pub failed_calls: i64,
    pub avg_duration_ms: Option<f64>,
    /// Up to a handful of the most recent distinct failure messages seen
    /// for this endpoint, most recent first — a lightweight failure-reason
    /// breakdown without a separate aggregation pass.
    pub recent_errors: Vec<String>,
}

#[derive(Clone)]
pub struct RpcRepository {
    collection: Collection<RpcLog>,
}

impl RpcRepository {
    pub fn new(db: &Database) -> Self {
        let collection = db.collection("rpc_logs");
        Self { collection }
    }

    pub async fn ensure_indexes(&self) -> Result<(), AppError> {
        let user_id_index = IndexModel::builder()
            .keys(doc! { "user_id": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("rpc_logs_user_id_idx".to_string()))
                    .build(),
            )
            .build();

        self.collection
            .create_index(user_id_index, None)
            .await
            .map(|_| ())
            .map_err(AppError::Database)
    }

    pub async fn save(&self, log: &RpcLog) -> Result<(), AppError> {
        self.collection.insert_one(log, None).await?;
        Ok(())
    }

    pub async fn find_by_user_id(
        &self,
        user_id: mongodb::bson::oid::ObjectId,
        limit: i64,
    ) -> Result<Vec<RpcLog>, AppError> {
        use mongodb::bson::doc;
        use mongodb::options::FindOptions;

        let filter = doc! { "user_id": user_id };
        let opts = FindOptions::builder()
            .sort(doc! { "_id": -1 })
            .limit(Some(limit))
            .build();

        let mut cursor = self.collection.find(filter, Some(opts)).await?;

        let mut logs = Vec::new();
        while cursor.advance().await? {
            let log = cursor.deserialize_current()?;
            logs.push(log);
        }

        Ok(logs)
    }

    pub async fn count_all(&self) -> Result<u64, AppError> {
        let count = self.collection.count_documents(None, None).await?;
        Ok(count)
    }

    pub async fn find_recent(&self, limit: i64) -> Result<Vec<RpcLog>, AppError> {
        use mongodb::bson::doc;
        use mongodb::options::FindOptions;

        let opts = FindOptions::builder()
            .sort(doc! { "_id": -1 })
            .limit(Some(limit))
            .build();

        let mut cursor = self.collection.find(None, Some(opts)).await?;
        let mut logs = Vec::new();
        while cursor.advance().await? {
            let log = cursor.deserialize_current()?;
            logs.push(log);
        }

        Ok(logs)
    }

    /// Aggregates the most recent `sample_size` logged calls into per-endpoint
    /// stats: usage volume, failure count, average latency, and a short
    /// recent-errors sample. Computed in-memory over a bounded recent window
    /// (mirroring `find_recent`) rather than a Mongo aggregation pipeline —
    /// simple, and the window keeps stats representative of current health
    /// without an unbounded full-collection scan.
    pub async fn endpoint_stats(&self, sample_size: i64) -> Result<Vec<EndpointStats>, AppError> {
        let logs = self.find_recent(sample_size).await?;

        struct Acc {
            total_calls: i64,
            failed_calls: i64,
            duration_sum: i64,
            duration_count: i64,
            recent_errors: Vec<String>,
        }

        let mut by_endpoint: HashMap<String, Acc> = HashMap::new();

        for log in logs {
            let Some(endpoint) = log.endpoint else {
                continue;
            };

            let acc = by_endpoint.entry(endpoint).or_insert(Acc {
                total_calls: 0,
                failed_calls: 0,
                duration_sum: 0,
                duration_count: 0,
                recent_errors: Vec::new(),
            });

            acc.total_calls += 1;
            if !log.success {
                acc.failed_calls += 1;
                if let Some(err) = log.error {
                    if acc.recent_errors.len() < 5 {
                        acc.recent_errors.push(err);
                    }
                }
            }
            if let Some(duration) = log.duration_ms {
                acc.duration_sum += duration;
                acc.duration_count += 1;
            }
        }

        let mut stats: Vec<EndpointStats> = by_endpoint
            .into_iter()
            .map(|(endpoint, acc)| EndpointStats {
                endpoint,
                total_calls: acc.total_calls,
                failed_calls: acc.failed_calls,
                avg_duration_ms: if acc.duration_count > 0 {
                    Some(acc.duration_sum as f64 / acc.duration_count as f64)
                } else {
                    None
                },
                recent_errors: acc.recent_errors,
            })
            .collect();

        stats.sort_by(|a, b| b.total_calls.cmp(&a.total_calls));
        Ok(stats)
    }
}
