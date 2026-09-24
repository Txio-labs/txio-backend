use crate::utils::error::AppError;
use std::net::IpAddr;
use tokio::net::lookup_host;
use url::{Host, Url};

/// Rejects a user-supplied URL that resolves to localhost, a private/
/// link-local address, or any non-HTTPS scheme — the general SSRF guard any
/// feature accepting an outbound URL from a user should apply (custom RPC
/// endpoints, webhook subscriptions, ...). Originally
/// `CollectionService::validate_url`; extracted here so new features reuse
/// the same checks instead of re-deriving them.
///
/// Note: there is an inherent TOCTOU window between this check and the
/// actual HTTP connect (DNS rebinding). Operators who need to close that
/// window fully should deploy an egress proxy enforcing IP allowlists at
/// the network layer.
pub async fn validate_https_url(url_str: &str) -> Result<(), AppError> {
    let url = Url::parse(url_str).map_err(|e| AppError::BadRequest(format!("Invalid URL: {e}")))?;

    if url.scheme() != "https" {
        return Err(AppError::BadRequest("Only HTTPS URLs are allowed".into()));
    }

    match url.host() {
        Some(Host::Domain(host)) if host.eq_ignore_ascii_case("localhost") => {
            return Err(AppError::BadRequest("Localhost URLs are not allowed".into()));
        }
        Some(Host::Ipv4(v4)) => {
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() {
                return Err(AppError::BadRequest(
                    "Private or link-local IP addresses are not allowed".into(),
                ));
            }
        }
        Some(Host::Ipv6(v6)) => {
            if v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local() {
                return Err(AppError::BadRequest(
                    "Private or link-local IP addresses are not allowed".into(),
                ));
            }
        }
        Some(Host::Domain(host)) => {
            let port = url.port().unwrap_or(443);
            let lookup_addr = format!("{host}:{port}");
            let addrs: Vec<_> = lookup_host(&lookup_addr)
                .await
                .map_err(|_| AppError::BadRequest(format!("URL hostname could not be resolved: {host}")))?
                .collect();

            if addrs.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "URL hostname resolved to no addresses: {host}"
                )));
            }

            for addr in addrs {
                let ip: IpAddr = addr.ip();
                let blocked = match ip {
                    IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
                    IpAddr::V6(v6) => {
                        v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
                    }
                };
                if blocked {
                    return Err(AppError::BadRequest(
                        "URL resolves to a private or link-local address".into(),
                    ));
                }
            }
        }
        None => {}
    }

    Ok(())
}
