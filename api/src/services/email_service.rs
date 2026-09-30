use crate::services::otp_service::OTP_VALIDITY_MINUTES;
use crate::utils::error::AppError;
use reqwest::Client;
use serde_json::json;

#[derive(Clone)]
pub struct EmailService {
    api_key: String,
    client: Client,
}

impl EmailService {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// The invitation email. `link` carries the single-use token in a URL
    /// fragment so it never reaches server logs.
    pub async fn send_workspace_invite_email(
        &self,
        email: &str,
        workspace_name: &str,
        inviter_email: &str,
        role: &str,
        link: &str,
    ) -> Result<(), AppError> {
        let from_email =
            std::env::var("EMAIL_FROM").unwrap_or_else(|_| "no-reply@txio-backend.com".to_string());
        let from_name =
            std::env::var("EMAIL_FROM_NAME").unwrap_or_else(|_| "txio Team".to_string());

        if self.api_key.trim().is_empty() {
            return Err(AppError::ExternalService("Email provider is not configured".into()));
        }
        let workspace_name = html_escape(workspace_name);
        let inviter_email = html_escape(inviter_email);
        let body = json!({
            "sender": { "email": from_email, "name": from_name },
            "to": [{ "email": email }],
            "subject": format!("{inviter_email} invited you to a txio workspace"),
            "htmlContent": format!(
                "<p>{inviter_email} invited you to <strong>{workspace_name}</strong> on txio as {role}.</p>\
                 <p><a href=\"{link}\">Accept the invitation</a></p>\
                 <p>The link works once and expires in 7 days. Sign in with this email address ({}) to accept. \
                 If you were not expecting this, ignore this email.</p>",
                html_escape(email)
            )
        });

        let response = self
            .client
            .post("https://api.brevo.com/v3/smtp/email")
            .header("api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("Failed to send email: {e}")))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!("Brevo API error: {error_text}")));
        }
        Ok(())
    }

    pub async fn send_otp_email(&self, email: &str, otp: &str) -> Result<(), AppError> {
        // Brevo only sends from verified senders, so the from-address must be
        // configurable per deployment instead of a hardcoded domain.
        let from_email =
            std::env::var("EMAIL_FROM").unwrap_or_else(|_| "no-reply@txio-backend.com".to_string());
        let from_name =
            std::env::var("EMAIL_FROM_NAME").unwrap_or_else(|_| "txio Team".to_string());

        let body = json!({
            "sender": { "email": from_email, "name": from_name },
            "to": [{ "email": email }],
            "subject": "Your txio OTP",
            "htmlContent": format!(
                "<p>Your verification code is: <strong>{}</strong></p><p>This code will expire in {} minutes.</p>",
                otp, OTP_VALIDITY_MINUTES
            )
        });

        let response = self
            .client
            .post("https://api.brevo.com/v3/smtp/email")
            .header("api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ExternalService(format!("Failed to send email: {e}")))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AppError::ExternalService(format!(
                "Brevo API error: {error_text}"
            )));
        }

        Ok(())
    }
}

/// Workspace and email values are user-controlled and end up inside HTML.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::html_escape;

    #[test]
    fn escapes_markup_in_user_supplied_names() {
        assert_eq!(html_escape("<script>\"x\"&"), "&lt;script&gt;&quot;x&quot;&amp;");
    }
}
