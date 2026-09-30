use crate::dtos::{
    request::{
        DeleteUserRequest, LoginRequest, OTPRequest, RegisterUserRequest, ResetPasswordWithOTPRequest,
        SwitchNetworkRequest, UpdateEmailRequest, UpdateNotificationPreferencesRequest,
        UpdatePasswordRequest, UpdateProfileRequest, VerifyOTPRequest,
    },
    response::UserResponse,
};
use crate::model::user::GitHubAccount;
use crate::services::auth_service::AuthService;
use crate::utils::error::AppError;
use crate::utils::session_cookie::{
    clear_oauth_nonce_cookie, clear_oauth_pkce_cookie, clear_session_cookie, oauth_nonce_cookie,
    oauth_pkce_cookie, read_cookie, session_cookie, OAUTH_NONCE_COOKIE, OAUTH_PKCE_COOKIE,
};
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

/// Persist a sessions-collection row for a freshly issued JWT. A login is only
/// valid while its row exists (see the `Claims` extractor), so a failure to
/// save it fails the login instead of issuing a token that would be rejected.
async fn record_login_session(
    service: &AuthService,
    token: &str,
    headers: &HeaderMap,
    addr: &SocketAddr,
) -> Result<(), AppError> {
    let claims = service.verify_token(token)?;
    let jti = claims
        .jti
        .as_deref()
        .filter(|j| !j.is_empty())
        .ok_or_else(|| AppError::InternalError("Issued token has no jti".into()))?;
    service
        .create_session(
            &claims.sub,
            jti,
            &device_label_from_headers(headers),
            &client_ip_from_request(headers, addr),
        )
        .await?;
    Ok(())
}

pub async fn register(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<RegisterUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let response = service.register_user(payload).await?;
    record_login_session(&service, &response.token, &headers, &addr).await?;

    Ok((
        [(header::SET_COOKIE, session_cookie(&response.token))],
        Json(response),
    ))
}

pub async fn login(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    let response = service.login_user(payload).await?;
    record_login_session(&service, &response.token, &headers, &addr).await?;

    Ok((
        [(header::SET_COOKIE, session_cookie(&response.token))],
        Json(response),
    ))
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

/// POST /auth/logout — deletes the caller's session row (so the JWT stops
/// working everywhere, not just in this browser) and clears the cookie.
/// Best-effort auth: logging out with an already-invalid session still
/// clears the cookie.
pub async fn logout(
    State(service): State<AuthService>,
    claims: Option<crate::utils::auth_jwt::Claims>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(jti) = claims.as_ref().and_then(|c| c.jti.as_deref()) {
        service.end_session(jti).await?;
    }
    Ok((
        [(header::SET_COOKIE, clear_session_cookie())],
        Json(json!({ "message": "Logged out successfully" })),
    ))
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

/// Sends the browser back to the app after a successful OAuth sign-in. The
/// session travels only in the HttpOnly cookie set on this same response; no
/// token is placed in the URL (URLs end up in history, logs and referrers).
fn oauth_success_response(frontend_url: &str, token: &str) -> axum::response::Response {
    let url = format!("{}/signin#oauth=success", frontend_url.trim_end_matches('/'));
    with_cookies(
        Redirect::to(&url),
        &[session_cookie(token), clear_oauth_nonce_cookie(), clear_oauth_pkce_cookie()],
    )
}

fn oauth_linked_response(frontend_url: &str, provider: &str) -> axum::response::Response {
    let url = format!(
        "{}/workspace#{provider}_connected=1",
        frontend_url.trim_end_matches('/')
    );
    with_cookies(Redirect::to(&url), &[clear_oauth_nonce_cookie(), clear_oauth_pkce_cookie()])
}

fn oauth_failure_response(frontend_url: &str, message: &str) -> axum::response::Response {
    with_cookies(
        oauth_error_redirect(frontend_url, message),
        &[clear_oauth_nonce_cookie(), clear_oauth_pkce_cookie()],
    )
}

fn with_cookies(redirect: Redirect, cookies: &[String]) -> axum::response::Response {
    let mut response = redirect.into_response();
    for cookie in cookies {
        if let Ok(value) = header::HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// Confirms the callback belongs to the browser that started the flow by
/// comparing the nonce inside the signed `state` with the nonce cookie.
fn require_matching_nonce(headers: &HeaderMap, state_nonce: &str) -> Result<(), AppError> {
    let cookie_nonce = read_cookie(headers, OAUTH_NONCE_COOKIE)
        .ok_or_else(|| AppError::BadRequest("Invalid or expired OAuth state".into()))?;
    if crate::services::otp_service::constant_time_eq(&cookie_nonce, state_nonce) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Invalid or expired OAuth state".into()))
    }
}

fn google_authorize_url(service: &AuthService, state: &str) -> Result<String, AppError> {
    let oauth_config = service
        .google_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Google sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/google/callback", service.backend_url.trim_end_matches('/'));
    Ok(format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=online&prompt=select_account",
        urlencoding::encode(&oauth_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(state),
    ))
}

fn github_authorize_url(service: &AuthService, state: &str) -> Result<String, AppError> {
    let oauth_config = service
        .github_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("GitHub sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/github/callback", service.backend_url.trim_end_matches('/'));
    Ok(format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&oauth_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("read:user user:email"),
        urlencoding::encode(state),
    ))
}

/// GET /auth/providers — which OAuth providers are configured, so the UI can
/// hide buttons that would only lead to an error.
pub async fn providers(State(service): State<AuthService>) -> Json<Value> {
    Json(json!({
        "google": service.google_oauth.is_some(),
        "github": service.github_oauth.is_some(),
        "x": service.x_oauth.is_some(),
    }))
}

/// GET /auth/google/login — standalone sign-in / sign-up.
pub async fn google_login(
    State(service): State<AuthService>,
) -> Result<axum::response::Response, AppError> {
    let (state, nonce) = generate_oauth_state(None)?;
    let url = google_authorize_url(&service, &state)?;
    Ok(with_cookies(Redirect::to(&url), &[oauth_nonce_cookie(&nonce)]))
}

/// POST /auth/google/link/start — authenticated; returns the provider URL for
/// attaching a Google identity to the caller's account. The account to link
/// is carried in the signed `state`, never a bearer token in a URL.
pub async fn google_link_start(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<impl IntoResponse, AppError> {
    let (state, nonce) = generate_oauth_state(Some(&claims.email))?;
    let url = google_authorize_url(&service, &state)?;
    Ok((
        [(header::SET_COOKIE, oauth_nonce_cookie(&nonce))],
        Json(json!({ "url": url })),
    ))
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
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<OAuthCallbackQuery>,
) -> axum::response::Response {
    match google_callback_inner(&service, &headers, query).await {
        Ok(OAuthOutcome::LoggedIn(token)) => {
            match record_login_session(&service, &token, &headers, &addr).await {
                Ok(()) => oauth_success_response(&service.frontend_url, &token),
                Err(e) => {
                    tracing::error!("Google OAuth session creation failed: {e}");
                    oauth_failure_response(&service.frontend_url, e.user_message())
                }
            }
        }
        Ok(OAuthOutcome::Linked) => oauth_linked_response(&service.frontend_url, "google"),
        Err(e) => {
            tracing::error!("Google OAuth callback failed: {e}");
            oauth_failure_response(&service.frontend_url, e.user_message())
        }
    }
}

/// Result of a provider callback.
enum OAuthOutcome {
    /// Standalone login/registration; carries the freshly issued JWT.
    LoggedIn(String),
    /// Provider identity attached to the already-authenticated user.
    Linked,
}

async fn google_callback_inner(
    service: &AuthService,
    headers: &HeaderMap,
    query: OAuthCallbackQuery,
) -> Result<OAuthOutcome, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!("Google sign-in was cancelled: {error}")));
    }
    let code = query
        .code
        .ok_or_else(|| AppError::BadRequest("Missing authorization code".into()))?;
    let state = query
        .state
        .ok_or_else(|| AppError::BadRequest("Missing OAuth state".into()))?;
    let oauth_state = verify_oauth_state(&state)?;
    require_matching_nonce(headers, &oauth_state.nonce)?;

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

    if let Some(link_email) = oauth_state.link_email {
        // Linking flow: attach this Google identity to the already
        // authenticated user rather than logging in/registering a new one.
        service
            .link_google_account(&link_email, claims.sub, claims.email)
            .await?;
        return Ok(OAuthOutcome::Linked);
    }

    let auth_response = service
        .oauth_login_or_register(claims.sub, claims.email)
        .await?;
    Ok(OAuthOutcome::LoggedIn(auth_response.token))
}

/// GET /auth/github/login — standalone sign-in / sign-up.
pub async fn github_login(
    State(service): State<AuthService>,
) -> Result<axum::response::Response, AppError> {
    let (state, nonce) = generate_oauth_state(None)?;
    let url = github_authorize_url(&service, &state)?;
    Ok(with_cookies(Redirect::to(&url), &[oauth_nonce_cookie(&nonce)]))
}

/// POST /auth/github/link/start — authenticated; see `google_link_start`.
pub async fn github_link_start(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<impl IntoResponse, AppError> {
    let (state, nonce) = generate_oauth_state(Some(&claims.email))?;
    let url = github_authorize_url(&service, &state)?;
    Ok((
        [(header::SET_COOKIE, oauth_nonce_cookie(&nonce))],
        Json(json!({ "url": url })),
    ))
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
}

#[derive(serde::Deserialize)]
struct GitHubEmailEntry {
    email: String,
    primary: bool,
    verified: bool,
}

pub async fn github_callback(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<OAuthCallbackQuery>,
) -> axum::response::Response {
    match github_callback_inner(&service, &headers, query).await {
        Ok(OAuthOutcome::LoggedIn(token)) => {
            match record_login_session(&service, &token, &headers, &addr).await {
                Ok(()) => oauth_success_response(&service.frontend_url, &token),
                Err(e) => {
                    tracing::error!("GitHub OAuth session creation failed: {e}");
                    oauth_failure_response(&service.frontend_url, e.user_message())
                }
            }
        }
        Ok(OAuthOutcome::Linked) => oauth_linked_response(&service.frontend_url, "github"),
        Err(e) => {
            tracing::error!("GitHub OAuth callback failed: {e}");
            oauth_failure_response(&service.frontend_url, e.user_message())
        }
    }
}

async fn github_callback_inner(
    service: &AuthService,
    headers: &HeaderMap,
    query: OAuthCallbackQuery,
) -> Result<OAuthOutcome, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!("GitHub sign-in was cancelled: {error}")));
    }
    let code = query
        .code
        .ok_or_else(|| AppError::BadRequest("Missing authorization code".into()))?;
    let state = query
        .state
        .ok_or_else(|| AppError::BadRequest("Missing OAuth state".into()))?;
    // `link_email` is set when this is a "connect to my account" flow started
    // from `link/start`; otherwise this is a standalone sign-in/sign-up.
    let oauth_state = verify_oauth_state(&state)?;
    require_matching_nonce(headers, &oauth_state.nonce)?;

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
        // Identity only: the provider token is not needed after this request.
        access_token: None,
    };

    if let Some(link_email) = oauth_state.link_email {
        service
            .link_github_account(&link_email, github_account)
            .await?;
        return Ok(OAuthOutcome::Linked);
    }

    // Standalone login/signup: the `/user` endpoint's `email` is null for
    // accounts with a private email, so fall back to `/user/emails` (which
    // the `user:email` scope grants) and prefer the verified primary address.
    // Only the verified primary address is trusted. The profile `email` field
    // is not guaranteed verified, and using it would let someone claim an
    // existing account by setting that address on their GitHub profile.
    let email = fetch_primary_github_email(&client, &access_token).await?;

    let auth_response = service
        .github_login_or_register(github_account, email)
        .await?;
    Ok(OAuthOutcome::LoggedIn(auth_response.token))
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

// ── X (Twitter) ─────────────────────────────────────────────────────────
//
// OAuth 2.0 authorization code flow with PKCE. X returns no email address, so
// this can sign in an account that already linked its X identity, or link X to
// the signed-in account; it never creates an account.

/// A PKCE verifier and its S256 challenge.
fn new_pkce() -> (String, String) {
    let bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn x_authorize_url(service: &AuthService, state: &str, challenge: &str) -> Result<String, AppError> {
    let oauth_config = service
        .x_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("X sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/x/callback", service.backend_url.trim_end_matches('/'));
    Ok(format!(
        "https://x.com/i/oauth2/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlencoding::encode(&oauth_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("users.read tweet.read"),
        urlencoding::encode(state),
        urlencoding::encode(challenge),
    ))
}

/// GET /auth/x/login — sign in with a linked X account.
pub async fn x_login(State(service): State<AuthService>) -> Result<axum::response::Response, AppError> {
    let (state, nonce) = generate_oauth_state(None)?;
    let (verifier, challenge) = new_pkce();
    let url = x_authorize_url(&service, &state, &challenge)?;
    Ok(with_cookies(Redirect::to(&url), &[oauth_nonce_cookie(&nonce), oauth_pkce_cookie(&verifier)]))
}

/// POST /auth/x/link/start — authenticated; returns the provider URL for
/// attaching an X identity to the caller's account.
pub async fn x_link_start(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<impl IntoResponse, AppError> {
    let (state, nonce) = generate_oauth_state(Some(&claims.email))?;
    let (verifier, challenge) = new_pkce();
    let url = x_authorize_url(&service, &state, &challenge)?;
    let mut response = Json(json!({ "url": url })).into_response();
    for cookie in [oauth_nonce_cookie(&nonce), oauth_pkce_cookie(&verifier)] {
        if let Ok(value) = header::HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

pub async fn x_unlink(
    State(service): State<AuthService>,
    claims: crate::utils::auth_jwt::Claims,
) -> Result<Json<Value>, AppError> {
    let user = service.get_user_profile_by_email(&claims.email).await?;
    if user.x_account.is_none() {
        return Err(AppError::BadRequest("No X account linked".into()));
    }
    service.unlink_x_account(&claims.email).await?;
    Ok(Json(json!({ "message": "X account unlinked successfully" })))
}

pub async fn x_callback(
    State(service): State<AuthService>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<OAuthCallbackQuery>,
) -> axum::response::Response {
    match x_callback_inner(&service, &headers, query).await {
        Ok(OAuthOutcome::LoggedIn(token)) => {
            match record_login_session(&service, &token, &headers, &addr).await {
                Ok(()) => oauth_success_response(&service.frontend_url, &token),
                Err(e) => {
                    tracing::error!("X OAuth session creation failed: {e}");
                    oauth_failure_response(&service.frontend_url, e.user_message())
                }
            }
        }
        Ok(OAuthOutcome::Linked) => oauth_linked_response(&service.frontend_url, "x"),
        Err(e) => {
            tracing::error!("X OAuth callback failed: {e}");
            oauth_failure_response(&service.frontend_url, e.user_message())
        }
    }
}

#[derive(serde::Deserialize)]
struct XTokenResponse {
    access_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct XUserEnvelope {
    data: XUser,
}

#[derive(serde::Deserialize)]
struct XUser {
    id: String,
    username: String,
}

async fn x_callback_inner(
    service: &AuthService,
    headers: &HeaderMap,
    query: OAuthCallbackQuery,
) -> Result<OAuthOutcome, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::BadRequest(format!("X sign-in was cancelled: {error}")));
    }
    let code = query.code.ok_or_else(|| AppError::BadRequest("Missing authorization code".into()))?;
    let state = query.state.ok_or_else(|| AppError::BadRequest("Missing OAuth state".into()))?;
    let oauth_state = verify_oauth_state(&state)?;
    require_matching_nonce(headers, &oauth_state.nonce)?;
    let verifier = read_cookie(headers, OAUTH_PKCE_COOKIE)
        .ok_or_else(|| AppError::BadRequest("Invalid or expired OAuth state".into()))?;

    let oauth_config = service
        .x_oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("X sign-in is not configured".into()))?;
    let redirect_uri = format!("{}/api/v1/auth/x/callback", service.backend_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let token_res = client
        .post("https://api.x.com/2/oauth2/token")
        .basic_auth(&oauth_config.client_id, Some(&oauth_config.client_secret))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier.as_str()),
            ("client_id", oauth_config.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("X token exchange failed: {e}")))?;
    if !token_res.status().is_success() {
        return Err(AppError::ExternalService("X rejected the authorization code".into()));
    }
    let token_body: XTokenResponse = token_res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid X token response: {e}")))?;
    let access_token = token_body
        .access_token
        .ok_or_else(|| AppError::ExternalService("X returned no access token".into()))?;

    let user_res = client
        .get("https://api.x.com/2/users/me")
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| AppError::ExternalService(format!("X profile fetch failed: {e}")))?;
    if !user_res.status().is_success() {
        return Err(AppError::ExternalService("X rejected the access token".into()));
    }
    let x_user: XUserEnvelope = user_res
        .json()
        .await
        .map_err(|e| AppError::ExternalService(format!("Invalid X profile response: {e}")))?;
    // Identity only; the provider token is dropped here and never stored.
    let account = crate::model::user::XAccount { id: x_user.data.id, username: x_user.data.username };

    if let Some(link_email) = oauth_state.link_email {
        service.link_x_account(&link_email, account).await?;
        return Ok(OAuthOutcome::Linked);
    }
    Ok(OAuthOutcome::LoggedIn(service.x_login(&account).await?.token))
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
) -> Result<impl IntoResponse, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    if !service
        .verify_otp(payload.new_email.clone(), payload.otp.clone())
        .await?
    {
        return Err(AppError::BadRequest("Invalid or expired OTP".into()));
    }

    let user = service
        .update_user_email_by_email(&claims.email, &payload.new_email)
        .await?;

    // Sessions and their JWTs carry the old email; end them all so the user
    // signs in again under the new address instead of hitting 404s.
    service.end_all_sessions(&claims.sub).await?;

    Ok((
        [(header::SET_COOKIE, clear_session_cookie())],
        Json(json!({ "user": user, "reauthenticate": true })),
    ))
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
    Json(payload): Json<DeleteUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    use validator::Validate;
    payload
        .validate()
        .map_err(|e| AppError::ValidationError(e.to_string()))?;

    if !service
        .verify_otp(claims.email.clone(), payload.otp)
        .await?
    {
        return Err(AppError::BadRequest("Invalid or expired OTP".into()));
    }

    let user = service.delete_user_by_email(&claims.email).await?;

    Ok((
        [(header::SET_COOKIE, clear_session_cookie())],
        Json(json!({ "user": user })),
    ))
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
fn generate_oauth_state(link_email: Option<&str>) -> Result<(String, String), AppError> {
    let nonce_bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let nonce = URL_SAFE_NO_PAD.encode(&nonce_bytes);
    let issued_at = chrono::Utc::now().timestamp();

    let payload = json!({
        "nonce": nonce,
        "issued_at": issued_at,
        "link_email": link_email,
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
    Ok((URL_SAFE_NO_PAD.encode(envelope_bytes), nonce))
}

/// Contents of a verified `state` value.
struct VerifiedOAuthState {
    nonce: String,
    /// Email of the signed-in user this flow should link to, if any.
    link_email: Option<String>,
}

/// Verifies a `state` value's signature and expiry.
fn verify_oauth_state(state: &str) -> Result<VerifiedOAuthState, AppError> {
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

    Ok(VerifiedOAuthState {
        nonce: payload["nonce"].as_str().ok_or_else(invalid)?.to_string(),
        link_email: payload["link_email"].as_str().map(str::to_string),
    })
}

#[cfg(test)]
mod oauth_state_tests {
    use super::*;

    #[test]
    fn state_round_trips_and_carries_link_email() {
        std::env::set_var("JWT_SECRET", "test-secret");
        let (state, nonce) = generate_oauth_state(Some("a@x.com")).unwrap();
        let v = verify_oauth_state(&state).unwrap();
        assert_eq!(v.nonce, nonce);
        assert_eq!(v.link_email.as_deref(), Some("a@x.com"));
        let (state, _) = generate_oauth_state(None).unwrap();
        assert!(verify_oauth_state(&state).unwrap().link_email.is_none());
    }

    #[test]
    fn tampered_state_is_rejected() {
        std::env::set_var("JWT_SECRET", "test-secret");
        let (state, _) = generate_oauth_state(None).unwrap();
        let mut bytes = state.into_bytes();
        let i = bytes.len() / 2;
        bytes[i] = if bytes[i] == b'A' { b'B' } else { b'A' };
        assert!(verify_oauth_state(&String::from_utf8(bytes).unwrap()).is_err());
    }
}

#[cfg(test)]
mod pkce_tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let (verifier, challenge) = new_pkce();
        assert_eq!(verifier.len(), 43, "32 random bytes in base64url");
        let expected = URL_SAFE_NO_PAD.encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        assert_ne!(new_pkce().0, verifier);
    }

    #[test]
    fn pkce_challenge_matches_the_rfc7636_test_vector() {
        // RFC 7636 test vector: verifier -> challenge.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
