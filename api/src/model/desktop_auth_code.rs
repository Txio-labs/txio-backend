use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// A one-time code that hands a browser sign-in over to Txio Desktop.
///
/// The desktop app opens the website with a PKCE `challenge`. Once the user is
/// signed in on the web and confirms, the API stores this row and the browser
/// passes the plain code to the desktop app (through a `txio-desktop://` link,
/// or by the user pasting it). The desktop app redeems the code together with
/// the PKCE verifier only it holds, so a leaked code is useless on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopAuthCode {
    #[serde(rename = "_id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    /// SHA-256 (hex) of the code. The plain code is never stored.
    pub code_hash: String,
    pub user_id: ObjectId,
    /// base64url(SHA-256(verifier)), as sent by the desktop app.
    pub challenge: String,
    #[serde(with = "mongodb::bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}
