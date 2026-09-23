use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

use crate::model::network::Network;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct GitHubAccount {
    pub id: String,
    pub login: String,
    #[serde(skip_serializing)]
    pub access_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub email: String,
    pub password_hash: String,
    /// User-chosen display name. Falls back to the email's local part
    /// (`to_user_response`) when unset, so every account has a sensible
    /// name even before the user visits their profile settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_sub: Option<String>,
    pub tier: PlanTier,
    #[serde(default)]
    pub network: Network,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub github_account: Option<GitHubAccount>,
    #[serde(default)]
    pub notification_preferences: NotificationPreferences,
    #[serde(default)]
    pub failed_login_attempts: i32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub locked_until: Option<DateTime<Utc>>,
    /// Durable admin privilege. Set only via out-of-band bootstrap — never by
    /// matching `claims.email` against `ADMIN_EMAILS` at request time.
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NotificationPreferences {
    pub email_digests: bool,
    pub email_security_alerts: bool,
    pub in_app_activity_alerts: bool,
    pub in_app_product_updates: bool,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            email_digests: true,
            email_security_alerts: true,
            in_app_activity_alerts: true,
            in_app_product_updates: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PlanTier {
    Free,
    Pro,
    Team,
}

impl User {
    pub fn new(email: String, password_hash: String) -> Self {
        Self {
            id: None,
            email,
            password_hash,
            display_name: None,
            google_sub: None,
            tier: PlanTier::Free,
            network: Network::default(),
            created_at: Utc::now(),
            github_account: None,
            notification_preferences: NotificationPreferences::default(),
            failed_login_attempts: 0,
            locked_until: None,
            is_admin: false,
        }
    }

    pub fn new_oauth(email: String, password_hash: String, google_sub: String) -> Self {
        Self {
            google_sub: Some(google_sub),
            ..Self::new(email, password_hash)
        }
    }

    pub fn new_github_oauth(
        email: String,
        password_hash: String,
        github_account: GitHubAccount,
    ) -> Self {
        Self {
            github_account: Some(github_account),
            ..Self::new(email, password_hash)
        }
    }
}
