use crate::dtos::admin_dtos::{
    AdminCollectionEntry, AdminOverviewResponse, AdminRequestEntry, AdminUserEntry,
};
use crate::utils::error::AppError;
use mongodb::bson::{doc, oid::ObjectId, Bson, DateTime as BsonDateTime, Document};
use mongodb::options::FindOptions;
use mongodb::{Collection, Database};
use std::collections::HashMap;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Collections keyed by `user_id` that are purged with the account. Saved
/// requests come before their collections so a partial failure never leaves
/// requests pointing at a deleted collection.
const USER_OWNED_COLLECTIONS: [&str; 5] = [
    "saved_requests",
    "collections",
    "history",
    "workspaces",
    "rpc_logs",
];

/// Cross-user queries backing the admin dashboard, plus the data purge for
/// admin-initiated account deletion.
///
/// Reads raw documents with explicit projections instead of the typed models:
/// sensitive fields (password hashes, OAuth tokens) are never loaded, and one
/// legacy document that no longer matches a model can't fail a whole listing.
#[derive(Clone)]
pub struct AdminRepository {
    db: Database,
}

impl AdminRepository {
    pub fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }

    fn coll(&self, name: &str) -> Collection<Document> {
        self.db.collection::<Document>(name)
    }

    async fn count(&self, name: &str, filter: Option<Document>) -> Result<u64, AppError> {
        Ok(self.coll(name).count_documents(filter, None).await?)
    }

    async fn find_docs(
        &self,
        name: &str,
        filter: Option<Document>,
        options: FindOptions,
    ) -> Result<Vec<Document>, AppError> {
        let mut cursor = self.coll(name).find(filter, options).await?;
        let mut docs = Vec::new();
        while cursor.advance().await? {
            docs.push(cursor.deserialize_current()?);
        }
        Ok(docs)
    }

    async fn aggregate_docs(
        &self,
        name: &str,
        pipeline: Vec<Document>,
    ) -> Result<Vec<Document>, AppError> {
        let mut cursor = self.coll(name).aggregate(pipeline, None).await?;
        let mut docs = Vec::new();
        while cursor.advance().await? {
            docs.push(cursor.deserialize_current()?);
        }
        Ok(docs)
    }

    pub async fn overview(&self) -> Result<AdminOverviewResponse, AppError> {
        let now = BsonDateTime::now().timestamp_millis();
        let week_ago = BsonDateTime::from_millis(now - 7 * DAY_MS);
        let day_ago = BsonDateTime::from_millis(now - DAY_MS);

        Ok(AdminOverviewResponse {
            users: self.count("users", None).await?,
            admins: self.count("users", Some(doc! { "is_admin": true })).await?,
            workspaces: self.count("workspaces", None).await?,
            collections: self.count("collections", None).await?,
            saved_requests: self.count("saved_requests", None).await?,
            history_entries: self.count("history", None).await?,
            rpc_logs: self.count("rpc_logs", None).await?,
            active_sessions: self.count("sessions", None).await?,
            signups_last_7d: self
                .count("users", Some(doc! { "created_at": { "$gte": week_ago } }))
                .await?,
            requests_last_24h: self
                .count("history", Some(doc! { "executed_at": { "$gte": day_ago } }))
                .await?,
            rpc_calls_last_24h: self
                .count("rpc_logs", Some(doc! { "timestamp": { "$gte": day_ago } }))
                .await?,
        })
    }

    /// Maps user ids to emails for joining activity rows to their owner.
    pub async fn emails_for(
        &self,
        ids: impl IntoIterator<Item = ObjectId>,
    ) -> Result<HashMap<ObjectId, String>, AppError> {
        let mut ids: Vec<ObjectId> = ids.into_iter().collect();
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let options = FindOptions::builder()
            .projection(doc! { "email": 1 })
            .build();
        let docs = self
            .find_docs("users", Some(doc! { "_id": { "$in": ids } }), options)
            .await?;

        Ok(docs
            .iter()
            .filter_map(|d| Some((d.get_object_id("_id").ok()?, str_field(d, "email")?)))
            .collect())
    }

    /// Removes everything a user owns (except the user document and sessions,
    /// which the service deletes around this). Returns how many documents
    /// were removed per collection.
    pub async fn purge_user_data(
        &self,
        user_id: ObjectId,
    ) -> Result<Vec<(&'static str, u64)>, AppError> {
        let mut removed = Vec::new();
        for name in USER_OWNED_COLLECTIONS {
            let result = self
                .coll(name)
                .delete_many(doc! { "user_id": user_id }, None)
                .await?;
            removed.push((name, result.deleted_count));
        }
        Ok(removed)
    }

    pub async fn list_users(&self, limit: i64) -> Result<Vec<AdminUserEntry>, AppError> {
        let options = FindOptions::builder()
            .sort(doc! { "created_at": -1 })
            .limit(limit)
            .projection(doc! {
                "email": 1,
                "display_name": 1,
                "created_at": 1,
                "is_admin": 1,
                "google_sub": 1,
                "github_account.login": 1,
                "tier": 1,
            })
            .build();
        let users = self.find_docs("users", None, options).await?;
        let ids: Vec<ObjectId> = users
            .iter()
            .filter_map(|u| u.get_object_id("_id").ok())
            .collect();

        let collection_counts: HashMap<ObjectId, u64> = self
            .aggregate_docs(
                "collections",
                vec![
                    doc! { "$match": { "user_id": { "$in": ids.clone() } } },
                    doc! { "$group": { "_id": "$user_id", "count": { "$sum": 1 } } },
                ],
            )
            .await?
            .iter()
            .filter_map(|d| Some((d.get_object_id("_id").ok()?, int_field(d, "count")? as u64)))
            .collect();

        let mut activity: HashMap<ObjectId, (u64, Option<String>)> = HashMap::new();
        for d in self
            .aggregate_docs(
                "history",
                vec![
                    doc! { "$match": { "user_id": { "$in": ids } } },
                    doc! { "$group": {
                        "_id": "$user_id",
                        "count": { "$sum": 1 },
                        "last": { "$max": "$executed_at" },
                    } },
                ],
            )
            .await?
        {
            if let Ok(id) = d.get_object_id("_id") {
                let count = int_field(&d, "count").unwrap_or(0) as u64;
                activity.insert(id, (count, date_field(&d, "last")));
            }
        }

        Ok(users
            .iter()
            .filter_map(|u| {
                let id = u.get_object_id("_id").ok()?;
                let (request_count, last_active_at) =
                    activity.get(&id).cloned().unwrap_or((0, None));
                Some(AdminUserEntry {
                    id: id.to_hex(),
                    email: str_field(u, "email")?,
                    name: str_field(u, "display_name"),
                    created_at: date_field(u, "created_at"),
                    is_admin: u.get_bool("is_admin").unwrap_or(false),
                    google_linked: matches!(u.get("google_sub"), Some(Bson::String(_))),
                    github_login: u
                        .get_document("github_account")
                        .ok()
                        .and_then(|g| str_field(g, "login")),
                    tier: str_field(u, "tier"),
                    collection_count: collection_counts.get(&id).copied().unwrap_or(0),
                    request_count,
                    last_active_at,
                })
            })
            .collect())
    }

    pub async fn recent_requests(&self, limit: i64) -> Result<Vec<AdminRequestEntry>, AppError> {
        let options = FindOptions::builder()
            .sort(doc! { "executed_at": -1 })
            .limit(limit)
            .projection(doc! { "params": 0 })
            .build();
        let docs = self.find_docs("history", None, options).await?;
        let emails = self
            .emails_for(docs.iter().filter_map(|d| d.get_object_id("user_id").ok()))
            .await?;

        Ok(docs
            .iter()
            .filter_map(|d| {
                Some(AdminRequestEntry {
                    id: d.get_object_id("_id").ok()?.to_hex(),
                    user_email: d
                        .get_object_id("user_id")
                        .ok()
                        .and_then(|id| emails.get(&id).cloned()),
                    name: str_field(d, "name").unwrap_or_default(),
                    request_type: str_field(d, "request_type"),
                    chain: str_field(d, "chain"),
                    network: str_field(d, "network"),
                    method: str_field(d, "method"),
                    status: int_field(d, "status"),
                    duration_ms: int_field(d, "duration_ms"),
                    executed_at: date_field(d, "executed_at"),
                })
            })
            .collect())
    }

    pub async fn list_collections(
        &self,
        limit: i64,
    ) -> Result<Vec<AdminCollectionEntry>, AppError> {
        let options = FindOptions::builder()
            .sort(doc! { "updated_at": -1 })
            .limit(limit)
            .build();
        let docs = self.find_docs("collections", None, options).await?;
        let collection_ids: Vec<ObjectId> = docs
            .iter()
            .filter_map(|d| d.get_object_id("_id").ok())
            .collect();

        let request_counts: HashMap<ObjectId, u64> = self
            .aggregate_docs(
                "saved_requests",
                vec![
                    doc! { "$match": { "collection_id": { "$in": collection_ids } } },
                    doc! { "$group": { "_id": "$collection_id", "count": { "$sum": 1 } } },
                ],
            )
            .await?
            .iter()
            .filter_map(|d| Some((d.get_object_id("_id").ok()?, int_field(d, "count")? as u64)))
            .collect();

        let emails = self
            .emails_for(docs.iter().filter_map(|d| d.get_object_id("user_id").ok()))
            .await?;

        Ok(docs
            .iter()
            .filter_map(|d| {
                let id = d.get_object_id("_id").ok()?;
                Some(AdminCollectionEntry {
                    id: id.to_hex(),
                    name: str_field(d, "name").unwrap_or_default(),
                    description: str_field(d, "description"),
                    owner_email: d
                        .get_object_id("user_id")
                        .ok()
                        .and_then(|uid| emails.get(&uid).cloned()),
                    request_count: request_counts.get(&id).copied().unwrap_or(0),
                    created_at: date_field(d, "created_at"),
                    updated_at: date_field(d, "updated_at"),
                })
            })
            .collect())
    }
}

fn str_field(d: &Document, key: &str) -> Option<String> {
    d.get_str(key).ok().map(str::to_string)
}

fn int_field(d: &Document, key: &str) -> Option<i64> {
    match d.get(key)? {
        Bson::Int32(v) => Some(i64::from(*v)),
        Bson::Int64(v) => Some(*v),
        Bson::Double(v) => Some(*v as i64),
        _ => None,
    }
}

fn date_field(d: &Document, key: &str) -> Option<String> {
    d.get_datetime(key).ok()?.try_to_rfc3339_string().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_field_accepts_every_numeric_bson_width() {
        let d = doc! { "a": 1_i32, "b": 2_i64, "c": 3.9_f64, "d": "x" };
        assert_eq!(int_field(&d, "a"), Some(1));
        assert_eq!(int_field(&d, "b"), Some(2));
        assert_eq!(int_field(&d, "c"), Some(3));
        assert_eq!(int_field(&d, "d"), None);
        assert_eq!(int_field(&d, "missing"), None);
    }

    #[test]
    fn date_field_formats_bson_datetime_as_rfc3339() {
        let d = doc! { "at": BsonDateTime::from_millis(0), "bad": "2024" };
        assert_eq!(date_field(&d, "at").as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(date_field(&d, "bad"), None);
    }
}
