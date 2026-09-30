//! Browser session cookie. The JWT is delivered to browsers as an `HttpOnly`
//! cookie so page JavaScript (and therefore any XSS) can never read it.
//! Non-browser clients (CLI, SDK, scripts) keep using `Authorization: Bearer`.

use axum::http::{header, HeaderMap};

pub const SESSION_COOKIE: &str = "txio_session";
/// Short-lived cookie binding an OAuth `state` value to the browser that
/// started the flow, so a callback URL cannot be replayed from another browser.
pub const OAUTH_NONCE_COOKIE: &str = "txio_oauth_nonce";
/// Holds the PKCE code verifier between the redirect to the provider and its callback.
pub const OAUTH_PKCE_COOKIE: &str = "txio_oauth_pkce";

const SESSION_MAX_AGE_SECONDS: i64 = 24 * 60 * 60;
const OAUTH_NONCE_MAX_AGE_SECONDS: i64 = 600;

fn attributes(max_age: i64, path: &str) -> String {
    // COOKIE_SECURE=false is meant for plain-http local development only.
    let secure = std::env::var("COOKIE_SECURE")
        .map(|v| !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    // Lax works when the app and API share a registrable domain. Set
    // COOKIE_SAMESITE=None (requires Secure) only for cross-site deployments.
    let same_site = match std::env::var("COOKIE_SAMESITE")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "none" if secure => "None",
        "strict" => "Strict",
        _ => "Lax",
    };
    let mut attrs = format!("; Path={path}; Max-Age={max_age}; HttpOnly; SameSite={same_site}");
    if secure {
        attrs.push_str("; Secure");
    }
    if let Ok(domain) = std::env::var("COOKIE_DOMAIN") {
        if !domain.trim().is_empty() {
            attrs.push_str(&format!("; Domain={}", domain.trim()));
        }
    }
    attrs
}

pub fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}{}", attributes(SESSION_MAX_AGE_SECONDS, "/"))
}

pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}={}", attributes(0, "/"))
}

pub fn oauth_nonce_cookie(nonce: &str) -> String {
    format!(
        "{OAUTH_NONCE_COOKIE}={nonce}{}",
        attributes(OAUTH_NONCE_MAX_AGE_SECONDS, "/api/v1/auth")
    )
}

pub fn oauth_pkce_cookie(verifier: &str) -> String {
    format!(
        "{OAUTH_PKCE_COOKIE}={verifier}{}",
        attributes(OAUTH_NONCE_MAX_AGE_SECONDS, "/api/v1/auth")
    )
}

pub fn clear_oauth_pkce_cookie() -> String {
    format!("{OAUTH_PKCE_COOKIE}={}", attributes(0, "/api/v1/auth"))
}

pub fn clear_oauth_nonce_cookie() -> String {
    format!("{OAUTH_NONCE_COOKIE}={}", attributes(0, "/api/v1/auth"))
}

/// Reads a cookie value from the request `Cookie` header(s).
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn reads_named_cookie() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; txio_session=tok.en; b=2"));
        assert_eq!(read_cookie(&h, SESSION_COOKIE).as_deref(), Some("tok.en"));
        assert_eq!(read_cookie(&h, "missing"), None);
    }

    #[test]
    fn session_cookie_is_httponly() {
        let c = session_cookie("t");
        assert!(c.contains("HttpOnly") && c.contains("Path=/") && c.starts_with("txio_session=t"));
    }

    #[test]
    fn clearing_expires_immediately() {
        assert!(clear_session_cookie().contains("Max-Age=0"));
    }
}
