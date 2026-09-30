//! Cross-cutting behaviour of `/api/public/v1`: per-key rate limiting,
//! request ids, and one error shape (JSON-RPC 2.0) for every failure.

use crate::repositories::api_key_repository::ApiKeyRepository;
use crate::utils::api_key_auth::{hash_api_key, ApiKeyAuth, API_KEY_PREFIX};
use crate::utils::rate_limit::{EndpointClass, KeyRateLimiter};
use axum::{
    body::to_bytes,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Extension,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

/// Application error codes. -32000/-32001/-32002 are the codes the rest of the
/// product already uses; the -3201x range is specific to this API and
/// documented in its OpenAPI description.
pub mod codes {
    pub const UPSTREAM_UNREACHABLE: i32 = -32000;
    pub const NAME_RESOLUTION_FAILED: i32 = -32001;
    pub const INTERNAL: i32 = -32002;
    pub const UNAUTHORIZED: i32 = -32010;
    pub const FORBIDDEN: i32 = -32011;
    pub const RATE_LIMITED: i32 = -32012;
    pub const INVALID_REQUEST: i32 = -32013;
    pub const NOT_FOUND: i32 = -32014;
    pub const NOT_IMPLEMENTED: i32 = -32015;
    pub const CONFLICT: i32 = -32016;
}

pub fn code_for_status(status: StatusCode) -> i32 {
    match status.as_u16() {
        401 => codes::UNAUTHORIZED,
        403 => codes::FORBIDDEN,
        404 => codes::NOT_FOUND,
        409 => codes::CONFLICT,
        429 => codes::RATE_LIMITED,
        501 => codes::NOT_IMPLEMENTED,
        502 | 503 | 504 => codes::UPSTREAM_UNREACHABLE,
        s if (400..500).contains(&s) => codes::INVALID_REQUEST,
        _ => codes::INTERNAL,
    }
}

/// The JSON-RPC 2.0 error object every failing response carries.
pub fn error_envelope(status: StatusCode, message: &str, request_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "error": {
            "code": code_for_status(status),
            "message": message,
            "data": { "status": status.as_u16(), "requestId": request_id },
        },
        "id": Value::Null,
    })
}

fn rate_headers(response: &mut Response, limit: u32, remaining: u32, reset_secs: u64) {
    let headers = response.headers_mut();
    for (name, value) in [
        ("x-ratelimit-limit", limit.to_string()),
        ("x-ratelimit-remaining", remaining.to_string()),
        ("x-ratelimit-reset", reset_secs.to_string()),
    ] {
        if let Ok(v) = HeaderValue::from_str(&value) {
            headers.insert(header::HeaderName::from_static(name), v);
        }
    }
}

/// Text of an error body produced by `AppError` (`{"error": "..."}`) or by an
/// extractor rejection (plain text).
fn message_from_body(bytes: &[u8]) -> String {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(bytes) {
        if let Some(Value::String(m)) = map.get("error") {
            return m.clone();
        }
    }
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        "Request failed".to_string()
    } else {
        text.chars().take(500).collect()
    }
}

pub async fn public_api_layer(
    Extension(limiter): Extension<Arc<KeyRateLimiter>>,
    Extension(keys): Extension<ApiKeyRepository>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut quota = None;

    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|k| k.starts_with(API_KEY_PREFIX))
        .map(str::to_string);

    // Resolve the key once here; the `ApiKeyAuth` extractor reuses it. A
    // missing, revoked or unknown key is left for the extractor to reject
    // with the right message (the app-wide per-IP limiter covers floods).
    if let Some(raw) = bearer {
        if let Ok(Some(key)) = keys.find_by_hash(&hash_api_key(&raw)).await {
            if let (true, Some(key_id)) = (key.is_active(), key.id) {
                let class = EndpointClass::for_path(request.uri().path());
                match limiter.check(key_id, class, Instant::now()) {
                    Ok(q) => quota = Some(q),
                    Err(retry_after) => {
                        let mut response = (
                            StatusCode::TOO_MANY_REQUESTS,
                            axum::Json(error_envelope(
                                StatusCode::TOO_MANY_REQUESTS,
                                "Rate limit exceeded for this API key",
                                &request_id,
                            )),
                        )
                            .into_response();
                        if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
                            response.headers_mut().insert(header::RETRY_AFTER, v);
                        }
                        rate_headers(&mut response, limiter.limit_for(class), 0, retry_after);
                        set_request_id(&mut response, &request_id);
                        return response;
                    }
                }
                request.extensions_mut().insert(ApiKeyAuth {
                    user_id: key.user_id,
                    api_key_id: key_id,
                    scopes: key.scopes,
                });
            }
        }
    }

    let response = next.run(request).await;
    let mut response = normalize_errors(response, &request_id).await;
    if let Some(q) = quota {
        rate_headers(&mut response, q.limit, q.remaining, q.reset_secs);
    }
    set_request_id(&mut response, &request_id);
    response
}

fn set_request_id(response: &mut Response, request_id: &str) {
    if let Ok(v) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(header::HeaderName::from_static("x-request-id"), v);
    }
}

/// Rewrites any 4xx/5xx body into the JSON-RPC envelope, so callers parse one shape.
async fn normalize_errors(response: Response, request_id: &str) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    // Already an envelope (the rate-limit response above is built directly).
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, 64 * 1024).await.unwrap_or_default();
    let message = message_from_body(&bytes);

    let mut rebuilt = (status, axum::Json(error_envelope(status, &message, request_id))).into_response();
    for name in [header::RETRY_AFTER, header::WWW_AUTHENTICATE] {
        if let Some(v) = parts.headers.get(&name) {
            rebuilt.headers_mut().insert(name, v.clone());
        }
    }
    rebuilt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_maps_to_documented_codes() {
        assert_eq!(code_for_status(StatusCode::UNAUTHORIZED), -32010);
        assert_eq!(code_for_status(StatusCode::FORBIDDEN), -32011);
        assert_eq!(code_for_status(StatusCode::TOO_MANY_REQUESTS), -32012);
        assert_eq!(code_for_status(StatusCode::BAD_REQUEST), -32013);
        assert_eq!(code_for_status(StatusCode::UNPROCESSABLE_ENTITY), -32013);
        assert_eq!(code_for_status(StatusCode::NOT_FOUND), -32014);
        assert_eq!(code_for_status(StatusCode::NOT_IMPLEMENTED), -32015);
        assert_eq!(code_for_status(StatusCode::CONFLICT), -32016);
        assert_eq!(code_for_status(StatusCode::BAD_GATEWAY), -32000);
        assert_eq!(code_for_status(StatusCode::INTERNAL_SERVER_ERROR), -32002);
    }

    #[test]
    fn envelope_is_jsonrpc_and_carries_request_id() {
        let v = error_envelope(StatusCode::FORBIDDEN, "no scope", "req-1");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["error"]["code"], -32011);
        assert_eq!(v["error"]["message"], "no scope");
        assert_eq!(v["error"]["data"]["requestId"], "req-1");
        assert!(v["id"].is_null());
    }

    #[test]
    fn extracts_messages_from_app_errors_and_plain_text() {
        assert_eq!(message_from_body(br#"{"error":"Invalid API key"}"#), "Invalid API key");
        assert_eq!(message_from_body(b"Failed to parse the request body as JSON"), "Failed to parse the request body as JSON");
        assert_eq!(message_from_body(b""), "Request failed");
    }

    #[tokio::test]
    async fn app_errors_and_extractor_rejections_become_envelopes() {
        use crate::utils::error::AppError;

        let forbidden = normalize_errors(AppError::Forbidden("no scope".into()).into_response(), "r1").await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let body: Value = serde_json::from_slice(&to_bytes(forbidden.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], -32011);
        assert_eq!(body["error"]["message"], "no scope");

        let limited = normalize_errors(AppError::TooManyRequests { retry_after_secs: 7 }.into_response(), "r2").await;
        assert_eq!(limited.headers().get(header::RETRY_AFTER).unwrap(), "7");

        let rejection = normalize_errors(
            (StatusCode::UNPROCESSABLE_ENTITY, "Failed to deserialize the JSON body").into_response(),
            "r3",
        )
        .await;
        let body: Value = serde_json::from_slice(&to_bytes(rejection.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], -32013);

        let ok = normalize_errors((StatusCode::OK, "fine").into_response(), "r4").await;
        assert_eq!(ok.status(), StatusCode::OK);
    }
}
