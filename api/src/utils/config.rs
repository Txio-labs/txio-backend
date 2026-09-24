use config::{Config as ConfigLoader, ConfigError, Environment};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub mongo_uri: String,
    pub jwt_secret: String,
    pub brevo_api_key: String,
    pub admin_emails: Vec<String>,
    pub google_oauth: Option<OAuthClientConfig>,
    pub github_oauth: Option<OAuthClientConfig>,
    pub backend_url: String,
    pub frontend_url: String,
    /// Symmetric key (AES-256-GCM) session keys' ephemeral automation
    /// signers are encrypted with at rest — see utils::session_key_crypto.
    /// Distinct from JWT_SECRET: rotating one must not force-rotate the
    /// other, since they protect very different things (login tokens vs.
    /// funds-capable automation keys).
    pub session_key_encryption_key: String,
}

/// Client credentials + redirect target for a single OAuth provider.
/// Optional at the app level: a deployment that hasn't configured a
/// provider simply won't offer that login/link option, rather than
/// failing to start.
#[derive(Debug, Deserialize, Clone)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let builder = ConfigLoader::builder().add_source(Environment::default());

        let config = builder.build()?;

        // Require critical values, no defaults for security
        let mongo_uri = config
            .get_string("MONGO_URI")
            .map_err(|_| ConfigError::Message("MONGO_URI must be set".into()))?;

        let jwt_secret = config
            .get_string("JWT_SECRET")
            .map_err(|_| ConfigError::Message("JWT_SECRET must be set".into()))?;

        let brevo_api_key = config
            .get_string("BREVO_API_KEY")
            .map_err(|_| ConfigError::Message("BREVO_API_KEY must be set".into()))?;

        if jwt_secret.len() < 32 {
            return Err(ConfigError::Message(
                "JWT_SECRET must be at least 32 characters".into(),
            ));
        }

        let session_key_encryption_key = config
            .get_string("SESSION_KEY_ENCRYPTION_KEY")
            .map_err(|_| ConfigError::Message("SESSION_KEY_ENCRYPTION_KEY must be set".into()))?;

        if session_key_encryption_key.len() < 32 {
            return Err(ConfigError::Message(
                "SESSION_KEY_ENCRYPTION_KEY must be at least 32 characters".into(),
            ));
        }

        let admin_emails = config
            .get_string("ADMIN_EMAILS")
            .map(|raw| parse_admin_emails(&raw))
            .unwrap_or_default();

        let backend_url = config
            .get_string("BACKEND_URL")
            .unwrap_or_else(|_| "http://localhost:8000".to_string());

        let frontend_url = config
            .get_string("FRONTEND_URL")
            .unwrap_or_else(|_| "http://localhost:3000".to_string());

        let google_oauth = oauth_client_from_env(&config, "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET");
        let github_oauth = oauth_client_from_env(&config, "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET");

        Ok(Config {
            mongo_uri,
            jwt_secret,
            brevo_api_key,
            admin_emails,
            google_oauth,
            github_oauth,
            backend_url,
            frontend_url,
            session_key_encryption_key,
        })
    }
}

/// Reads a provider's client id/secret pair from the environment. Returns
/// `None` (provider disabled) unless both values are present and non-empty —
/// a half-configured pair is treated the same as neither being set, rather
/// than starting up with a client_secret of "".
fn oauth_client_from_env(
    config: &config::Config,
    id_key: &str,
    secret_key: &str,
) -> Option<OAuthClientConfig> {
    let client_id = config.get_string(id_key).ok()?;
    let client_secret = config.get_string(secret_key).ok()?;
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return None;
    }
    Some(OAuthClientConfig {
        client_id,
        client_secret,
    })
}

/// Parses a comma-separated ADMIN_EMAILS env var into a normalized
/// (trimmed, lower-cased, empty entries dropped) list of email addresses.
///
/// These addresses are **reserved**: they cannot be claimed via self-service
/// registration / OAuth signup / email change. Admin privilege itself lives on
/// `User.is_admin` and is granted only via `bootstrap_admin`.
pub fn parse_admin_emails(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Returns true when `email` is on the reserved ADMIN_EMAILS roster.
pub fn is_reserved_admin_email(email: &str, reserved: &[String]) -> bool {
    let normalized = email.trim().to_ascii_lowercase();
    reserved
        .iter()
        .any(|admin_email| admin_email.eq_ignore_ascii_case(&normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_admin_emails() {
        let emails =
            parse_admin_emails(" Admin@Example.com ,, second@example.com,THIRD@EXAMPLE.COM");
        assert_eq!(
            emails,
            vec![
                "admin@example.com".to_string(),
                "second@example.com".to_string(),
                "third@example.com".to_string(),
            ]
        );
    }

    #[test]
    fn empty_admin_emails_yields_empty_list() {
        assert!(parse_admin_emails("").is_empty());
        assert!(parse_admin_emails("   ").is_empty());
    }

    #[test]
    fn reserved_email_match_is_case_insensitive() {
        let reserved = parse_admin_emails("Admin@Txio.io");
        assert!(is_reserved_admin_email("admin@txio.io", &reserved));
        assert!(is_reserved_admin_email(" ADMIN@TXIO.IO ", &reserved));
        assert!(!is_reserved_admin_email("user@txio.io", &reserved));
    }
}
