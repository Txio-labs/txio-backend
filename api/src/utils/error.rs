use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] mongodb::error::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    InternalError(String),

    #[error("External service error: {0}")]
    ExternalService(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Too many requests")]
    TooManyRequests { retry_after_secs: u64 },

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Conflict: {0}")]
    Conflict(String),
}

impl AppError {
    /// The status code and user-facing message for this error, in one place
    /// so redirect-based flows (OAuth callbacks) and JSON responses stay
    /// consistent — internals (DB/config/internal errors) are logged but
    /// never leaked to the client either way.
    pub fn status_and_message(&self) -> (StatusCode, &str) {
        match self {
            AppError::Database(e) => {
                tracing::error!("Database error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
            AppError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Configuration error"),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.as_str()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.as_str()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.as_str()),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.as_str()),
            AppError::ValidationError(msg) => (StatusCode::BAD_REQUEST, msg.as_str()),
            AppError::InternalError(msg) => {
                tracing::error!("Internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
            AppError::TooManyRequests { .. } => (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"),
            AppError::NotImplemented(msg) => (StatusCode::NOT_IMPLEMENTED, msg.as_str()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.as_str()),
            AppError::ExternalService(msg) => {
                tracing::error!("External service error: {}", msg);
                (StatusCode::BAD_GATEWAY, "External service unavailable")
            }
        }
    }

    /// The user-facing message alone, e.g. for building an OAuth error
    /// redirect where there's no JSON body to carry a status code.
    pub fn user_message(&self) -> &str {
        self.status_and_message().1
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = self.status_and_message();
        let retry_after = match &self {
            AppError::TooManyRequests { retry_after_secs } => Some(*retry_after_secs),
            _ => None,
        };

        let body = Json(json!({
            "error": error_message,
        }));

        let mut response = (status, body).into_response();
        if let Some(secs) = retry_after {
            if let Ok(value) = secs.to_string().parse() {
                response.headers_mut().insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
    }
}
