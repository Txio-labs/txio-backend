use crate::dtos::{
    request::{
        LoginRequest, OTPRequest, RegisterUserRequest, ResetPasswordWithOTPRequest,
        SwitchNetworkRequest, UpdateEmailRequest, UpdateNotificationPreferencesRequest,
        UpdatePasswordRequest, UpdateProfileRequest, VerifyOTPRequest,
    },
    response::{AuthResponse, UserResponse},
};
use crate::model::user::GitHubAccount;
use crate::services::auth_service::AuthService;
use crate::utils::error::AppError;
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Redirect},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::net::SocketAddr;

type HmacSha256 = Hmac<Sha256>;

/// Derive a short "Browser on OS" label from User-Agent for the sessions UI.
fn device_label_from_headers(headers: &HeaderMap) -> String {
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ua.is_empty() {
        return "Unknown device".to_string();
    }

    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };

    let os = if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Mac OS") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Unknown OS"
    };

    format!("{browser} on {os}")
}

/// Prefer reverse-proxy headers, then fall back to the TCP peer address.
fn client_ip_from_request(headers: &HeaderMap, addr: &SocketAddr) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if !real.is_empty() {
            return real.to_string();
        }
    }
    addr.ip().to_string()
}

/// Persist a sessions-collection row for a freshly issued JWT (best-effort).
async fn record_login_session(
    service: &AuthService,
    token: &str,
    headers: &HeaderMap,
    addr: &SocketAddr,
) {
    let Ok(claims) = service.verify_token(token) else {
        return;
    };
    let Some(jti) = claims.jti.as_deref().filter(|j| !j.is_empty()) else {
        return;
    };
    let _ = service
        .create_session(
            &claims.sub,
            jti,
            &device_label_from_headers(headers),
            &client_ip_from_request(headers, addr),
        )
        .await;
}

pub async fn register(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<RegisterUserRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let response = service.register_user(payload).await?;
    record_login_session(&service, &response.token, &headers, &addr).await;

    Ok(Json(response))
}

pub async fn login(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let response = service.login_user(payload).await?;
    record_login_session(&service, &response.token, &headers, &addr).await;

    Ok(Json(response))
}

/// GET /auth/sessions — list active sessions for the authenticated user.
pub async fn list_sessions(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let sessions = service
        .list_sessions(&claims.sub, claims.jti.as_deref())
        .await?;
    Ok(Json(json!({"sessions": sessions})))
}

/// DELETE /auth/sessions/:session_id — revoke one session owned by the caller.
pub async fn revoke_session(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    service.revoke_session(&claims.sub, &session_id).await?;
    Ok(Json(json!({ "message": "Session revoked" })))
}

pub async fn request_otp(
    State(service): State<AuthService>,
    Json(payload): Json<OTPRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    service.request_otp(payload.email).await?;

    Ok(Json(json!({ "message": "OTP sent successfully" })))
}

pub async fn verify_otp(
    State(service): State<AuthService>,
    Json(payload): Json<VerifyOTPRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let is_valid = service.verify_otp(payload.email, payload.otp).await?;

    if !is_valid {
        return Err(AppError::BadRequest("Invalid or expired OTP".into()));
    }

    Ok(Json(json!({ "message": "OTP verified successfully" })))
}

pub async fn profile(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<UserResponse>, AppError> {
    let user = service.get_user_profile_by_email(&claims.email).await?;

    Ok(Json(user))
}

pub async fn logout() -> Result<Json<Value>, AppError> {
    Ok(Json(json!({ "message": "Logged out successfully" })))
}

pub async fn github_unlink(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let user = service.get_user_profile_by_email(&claims.email).await?;

    if user.github_account.is_none() {
        return Err(AppError::BadRequest("No GitHub account linked".into()));
    }

    service
        .update_user_github_account(&claims.email, None)
        .await?;

    Ok(Json(
        json!({ "message": "GitHub account unlinked successfully" }),
    ))
}

#[derive(serde::Deserialize)]
pub struct OAuthLoginQuery {
    /// Present only when this is a "connect existing account" flow (e.g. the
    /// GitHub/Google "Connect" buttons on the profile page), carrying the
    /// requesting user's own JWT so the callback knows who to link to. A
    /// raw browser redirect can't carry an Authorization header, so this is
    /// smuggled through the signed `state` round-trip instead.
    pub link_token: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

fn oauth_error_redirect(frontend_url: &str, message: &str) -> Redirect {
    let url = format!(
        "{}/signin#oauth_error={}",
        frontend_url.trim_end_matches('/'),
        urlencoding::encode(message)
    );
    Redirect::to(&url)
}

fn oauth_success_redirect(frontend_url: &str, token: &str) -> Redirect {
    let url = format!(
        "{}/signin#token={}",
        frontend_url.trim_end_matches('/'),
        urlencoding::encode(token)
    );
    Redirect::to(&url)
}

pub async fn google_login(
    State(service): State<AuthService>,
    Query(query): Query<OAuthLoginQuery>,
) -> Result<Redirect, AppError> {
    let oauth_config = service
        .google_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Google sign-in is not configured".into()))?;

    let state = generate_oauth_state(query.link_token.as_deref())?;
    let redirect_uri = format!("{}/api/v1/auth/google/callback", service.backend_url.trim_end_matches('/'));

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=online&prompt=select_account",
        urlencoding::encode(&oauth_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(&state),
    );

    Ok(Redirect::to(&auth_url))
}

#[derive(serde::Deserialize)]
struct GoogleTokenResponse {
    id_token: String,
}

#[derive(serde::Deserialize)]
struct GoogleIdTokenClaims {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: bool,
}

pub async fn google_callback(
    State(service): State<AuthService>,
    Query(query): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    match google_callback_inner(&service, query).await {
        Ok(token) => oauth_success_redirect(&service.frontend_url, &token),
        Err(e) => {
            tracing::error!("Google OAuth callback failed: {e}");
            oauth_error_redirect(&service.frontend_url, e.user_message())
        }
    }
}

async fn google_callback_inner(
    service: &AuthService,
    query: OAuthCallbackQuery,
) -> Result<String, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!("Google sign-in was cancelled: {error}")));
    }
    let code = query
        .code
        .ok_or_else(|| AppError::BadRequest("Missing authorization code".into()))?;
    let state = query
        .state
        .ok_or_else(|| AppError::BadRequest("Missing OAuth state".into()))?;
    let link_token = verify_oauth_state(&state)?;

    let oauth_config = service
        .google_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Google sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/google/callback", service.backend_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let token_res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", oauth_config.client_id.as_str()),
            ("client_secret", oauth_config.client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("Google token exchange failed: {e}")))?;

    if !token_res.status().is_success() {
        return Err(AppError::ExternalService(
            "Google rejected the authorization code".into(),
        ));
    }

    let token_body: GoogleTokenResponse = token_res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid Google token response: {e}")))?;

    // The id_token is a JWT signed by Google; we only need its payload here
    // (the initial token exchange over TLS with our own client_secret is
    // what actually authenticates this request to Google — decoding the
    // id_token without verifying its signature is standard practice for
    // extracting claims already implicitly trusted via that exchange).
    let claims = decode_jwt_payload_unverified::<GoogleIdTokenClaims>(&token_body.id_token)
        .map_err(|_| AppError::ExternalService("Invalid Google identity token".into()))?;

    if !claims.email_verified {
        return Err(AppError::Unauthorized(
            "Google account email is not verified".into(),
        ));
    }

    if let Some(link_token) = link_token {
        // Linking flow: attach this Google identity to the already
        // authenticated user rather than logging in/registering a new one.
        let existing_claims = service.verify_token(&link_token)?;
        service
            .link_google_account(&existing_claims.email, claims.sub, claims.email)
            .await?;
        return Ok(link_token);
    }

    let auth_response = service
        .oauth_login_or_register(claims.sub, claims.email)
        .await?;
    Ok(auth_response.token)
}

pub async fn github_login(
    State(service): State<AuthService>,
    Query(query): Query<OAuthLoginQuery>,
) -> Result<Redirect, AppError> {
    let oauth_config = service
        .github_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("GitHub sign-in is not configured".into()))?;

    // `link_token` is present when this is the profile page's "Connect"
    // flow (attach GitHub to the already-signed-in user) and absent for a
    // standalone "Sign in/up with GitHub" flow — both are valid entry points.
    let state = generate_oauth_state(query.link_token.as_deref())?;
    let redirect_uri = format!("{}/api/v1/auth/github/callback", service.backend_url.trim_end_matches('/'));

    let auth_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&oauth_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("read:user user:email"),
        urlencoding::encode(&state),
    );

    Ok(Redirect::to(&auth_url))
}

#[derive(serde::Deserialize)]
struct GitHubTokenResponse {
    access_token: Option<String>,
    error_description: Option<String>,
}

#[derive(serde::Deserialize)]
struct GitHubUserResponse {
    id: u64,
    login: String,
    email: Option<String>,
}

#[derive(serde::Deserialize)]
struct GitHubEmailEntry {
    email: String,
    primary: bool,
    verified: bool,
}

enum GitHubCallbackOutcome {
    /// Standalone login/registration — carries a fresh JWT for the browser.
    LoggedIn(String),
    /// Linked to the already-authenticated user from `link_token`.
    Linked,
}

pub async fn github_callback(
    State(service): State<AuthService>,
    Query(query): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    match github_callback_inner(&service, query).await {
        Ok(GitHubCallbackOutcome::LoggedIn(token)) => {
            oauth_success_redirect(&service.frontend_url, &token)
        }
        Ok(GitHubCallbackOutcome::Linked) => Redirect::to(&format!(
            "{}/workspace#github_connected=1",
            service.frontend_url.trim_end_matches('/')
        )),
        Err(e) => {
            tracing::error!("GitHub OAuth callback failed: {e}");
            oauth_error_redirect(&service.frontend_url, e.user_message())
        }
    }
}

async fn github_callback_inner(
    service: &AuthService,
    query: OAuthCallbackQuery,
) -> Result<GitHubCallbackOutcome, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!("GitHub sign-in was cancelled: {error}")));
    }
    let code = query
        .code
        .ok_or_else(|| AppError::BadRequest("Missing authorization code".into()))?;
    let state = query
        .state
        .ok_or_else(|| AppError::BadRequest("Missing OAuth state".into()))?;
    // `Some(link_token)` means "connect to my already-open session";
    // `None` means this is a standalone sign-in/sign-up attempt.
    let link_token = verify_oauth_state(&state)?;

    let oauth_config = service
        .github_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("GitHub sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/github/callback", service.backend_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let token_res = client
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", oauth_config.client_id.as_str()),
            ("client_secret", oauth_config.client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("GitHub token exchange failed: {e}")))?;

    let token_body: GitHubTokenResponse = token_res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid GitHub token response: {e}")))?;

    let access_token = token_body.access_token.ok_or_else(|| {
        AppError::ExternalService(
            token_body
                .error_description
                .unwrap_or_else(|| "GitHub rejected the authorization code".to_string()),
        )
    })?;

    let user_res = client
        .get("https://api.github.com/user")
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header(header::USER_AGENT, "txio-backend")
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("GitHub profile fetch failed: {e}")))?;

    if !user_res.status().is_success() {
        return Err(AppError::ExternalService(
            "GitHub rejected the access token".into(),
        ));
    }

    let github_user: GitHubUserResponse = user_res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid GitHub profile response: {e}")))?;

    let github_account = GitHubAccount {
        id: github_user.id.to_string(),
        login: github_user.login,
        access_token: Some(access_token.clone()),
    };

    if let Some(link_token) = link_token {
        let existing_claims = service.verify_token(&link_token)?;
        service
            .update_user_github_account(&existing_claims.email, Some(github_account))
            .await?;
        return Ok(GitHubCallbackOutcome::Linked);
    }

    // Standalone login/signup: the `/user` endpoint's `email` is null for
    // accounts with a private email, so fall back to `/user/emails` (which
    // the `user:email` scope grants) and prefer the verified primary address.
    let email = match github_user.email {
        Some(email) => email,
        None => fetch_primary_github_email(&client, &access_token).await?,
    };

    let auth_response = service
        .github_login_or_register(github_account, email)
        .await?;
    Ok(GitHubCallbackOutcome::LoggedIn(auth_response.token))
}

async fn fetch_primary_github_email(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<String, AppError> {
    let res = client
        .get("https://api.github.com/user/emails")
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header(header::USER_AGENT, "txio-backend")
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("GitHub email fetch failed: {e}")))?;

    if !res.status().is_success() {
        return Err(AppError::ExternalService(
            "GitHub rejected the access token".into(),
        ));
    }

    let emails: Vec<GitHubEmailEntry> = res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid GitHub email response: {e}")))?;

    emails
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email)
        .ok_or_else(|| {
            AppError::BadRequest(
                "Your GitHub account has no verified email. Add one before signing in.".into(),
            )
        })
}

/// Decodes the payload of a JWT without verifying its signature. Only used
/// for a provider's `id_token` immediately after exchanging an authorization
/// code for it over TLS with our own client_secret — that exchange is what
/// authenticates the token to us, not this decode step.
fn decode_jwt_payload_unverified<T: serde::de::DeserializeOwned>(
    token: &str,
) -> Result<T, AppError> {
    let payload_segment = token
        .split('.')
        .nth(1)
        .ok_or_else(|| AppError::ExternalService("Malformed identity token".into()))?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_segment)
        .map_err(|_| AppError::ExternalService("Malformed identity token".into()))?;
    serde_json::from_slice(&decoded)
        .map_err(|_| AppError::ExternalService("Malformed identity token".into()))
}

pub async fn get_user_profile(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let user = service.get_user_profile_by_email(&claims.email).await?;

    Ok(Json(json!({ "user": user })))
}

pub async fn update_user_email(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Json(payload): Json<UpdateEmailRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user = service
        .update_user_email_by_email(&claims.email, &payload.new_email)
        .await?;

    Ok(Json(json!({ "user": user })))
}

pub async fn update_notification_preferences(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Json(payload): Json<UpdateNotificationPreferencesRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user = service
        .update_notification_preferences_by_email(&claims.email, payload.notification_preferences)
        .await?;

    Ok(Json(json!({ "user": user })))
}

pub async fn update_user_password(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Json(payload): Json<UpdatePasswordRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user = service
        .update_user_password_by_email(
            &claims.email,
            &payload.current_password,
            &payload.new_password,
        )
        .await?;

    Ok(Json(json!({ "user": user })))
}

pub async fn delete_user(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let user = service.delete_user_by_email(&claims.email).await?;

    Ok(Json(json!({ "user": user })))
}

pub async fn forgot_password(
    State(service): State<AuthService>,
    Json(payload): Json<OTPRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    service.request_otp(payload.email).await?;

    Ok(Json(
        json!({ "message": "OTP for password reset sent successfully" }),
    ))
}

pub async fn reset_password_with_otp(
    State(service): State<AuthService>,
    Json(payload): Json<ResetPasswordWithOTPRequest>,
) -> Result<Json<Value>, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    service
        .reset_password_with_otp(&payload.email, &payload.otp, &payload.new_password)
        .await?;

    Ok(Json(json!({ "message": "Password reset successfully" })))
}

pub async fn log_rpc_call(
    State(_service): State<AuthService>,
    _claims: crate::utils::auth_jwt::Claims,
    Json(_payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Err(AppError::BadRequest("RPC logging is disabled".into()))
}

pub async fn get_rpc_history(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let logs = service.get_rpc_history(&claims.email).await?;
    Ok(Json(json!({ "history": logs })))
}

pub async fn switch_network(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Json(payload): Json<SwitchNetworkRequest>,
) -> Result<Json<Value>, AppError> {
    use mongodb::bson::oid::ObjectId;
    use std::str::FromStr;
    use validator::Validate;

    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::InternalError("Invalid user ID in token".into()))?;

    let user = service
        .update_user_network(user_id, payload.network)
        .await?;

    Ok(Json(json!({
        "message": "Network switched successfully",
        "user": user
    })))
}

pub async fn update_profile(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<Json<Value>, AppError> {
    use mongodb::bson::oid::ObjectId;
    use std::str::FromStr;
    use validator::Validate;

    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let user_id = ObjectId::from_str(&claims.sub)
        .map_err(|_| AppError::InternalError("Invalid user ID in token".into()))?;

    let user = service
        .update_display_name(user_id, payload.name.trim().to_string())
        .await?;

    Ok(Json(json!({
        "message": "Profile updated successfully",
        "user": user
    })))
}

/// How long an OAuth `state` value remains valid. Generous enough for a user
/// to actually go through a provider's consent screen, tight enough that a
/// leaked/logged state value stops being useful quickly.
const OAUTH_STATE_TTL_SECONDS: i64 = 600;

fn oauth_signing_key() -> Result<Vec<u8>, AppError> {
    let secret = std::env::var("JWT_SECRET")
        .map_err(|_| AppError::InternalError("JWT_SECRET not set".into()))?;
    Ok(secret.into_bytes())
}

/// Generates a signed, tamper-proof `state` value for an OAuth authorization
/// request. `link_token` is `Some` when this flow is linking a provider
/// account to an already-authenticated user (carried here because a plain
/// redirect can't attach an `Authorization` header) and `None` for a
/// standalone login flow. The provider echoes `state` back verbatim on
/// callback, where `verify_oauth_state` checks the signature and expiry.
fn generate_oauth_state(link_token: Option<&str>) -> Result<String, AppError> {
    let nonce: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let issued_at = chrono::Utc::now().timestamp();

    let payload = json!({
        "nonce": URL_SAFE_NO_PAD.encode(&nonce),
        "issued_at": issued_at,
        "link_token": link_token,
    });
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|_| AppError::InternalError("Failed to encode OAuth state".into()))?;

    let key = oauth_signing_key()?;
    let mut mac = HmacSha256::new_from_slice(&key)
        .map_err(|_| AppError::InternalError("HMAC key error".into()))?;
    mac.update(&payload_bytes);
    let signature = mac.finalize().into_bytes();

    let envelope = json!({
        "payload": URL_SAFE_NO_PAD.encode(&payload_bytes),
        "signature": URL_SAFE_NO_PAD.encode(signature),
    });
    let envelope_bytes = serde_json::to_vec(&envelope)
        .map_err(|_| AppError::InternalError("Failed to encode OAuth state".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(envelope_bytes))
}

/// Verifies a `state` value's signature and expiry, returning the
/// `link_token` it carries (if this was a link flow rather than a login).
fn verify_oauth_state(state: &str) -> Result<Option<String>, AppError> {
    let invalid = || AppError::BadRequest("Invalid or expired OAuth state".into());

    let envelope_bytes = URL_SAFE_NO_PAD.decode(state).map_err(|_| invalid())?;
    let envelope: Value = serde_json::from_slice(&envelope_bytes).map_err(|_| invalid())?;

    let payload_b64 = envelope["payload"].as_str().ok_or_else(invalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(envelope["signature"].as_str().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| invalid())?;

    let key = oauth_signing_key()?;
    let mut mac = HmacSha256::new_from_slice(&key)
        .map_err(|_| AppError::InternalError("HMAC key error".into()))?;
    mac.update(&payload_bytes);
    mac.verify_slice(&signature).map_err(|_| invalid())?;

    let payload: Value = serde_json::from_slice(&payload_bytes).map_err(|_| invalid())?;
    let issued_at = payload["issued_at"].as_i64().ok_or_else(invalid)?;
    if chrono::Utc::now().timestamp() - issued_at > OAUTH_STATE_TTL_SECONDS {
        return Err(invalid());
    }

    Ok(payload["link_token"].as_str().map(str::to_string))
}
