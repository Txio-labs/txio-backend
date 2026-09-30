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
            if is_blocked_ip(IpAddr::V4(v4)) {
                return Err(AppError::BadRequest(
                    "Private or link-local IP addresses are not allowed".into(),
                ));
            }
        }
        Some(Host::Ipv6(v6)) => {
            if is_blocked_ip(IpAddr::V6(v6)) {
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

            if addrs.iter().any(|addr| is_blocked_ip(addr.ip())) {
                return Err(AppError::BadRequest(
                    "URL resolves to a private or link-local address".into(),
                ));
            }
        }
        None => {}
    }

    Ok(())
}

/// True for any address a user-supplied outbound URL must never reach:
/// loopback, private, link-local, unspecified, CGNAT (100.64.0.0/10),
/// broadcast, multicast, documentation ranges, and IPv4 addresses embedded in
/// IPv6 (`::ffff:a.b.c.d`, which would otherwise bypass the IPv4 checks).
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    }
}

fn is_blocked_v4(v4: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_documentation()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        is_blocked_ip(s.parse().unwrap())
    }

    #[test]
    fn blocks_internal_ranges() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254",
            "0.0.0.0", "100.64.0.1", "100.127.255.255", "255.255.255.255", "224.0.0.1",
            "::1", "::", "fc00::1", "fe80::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254",
        ] {
            assert!(blocked(ip), "{ip} should be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ip in ["1.1.1.1", "8.8.8.8", "100.63.0.1", "2606:4700:4700::1111"] {
            assert!(!blocked(ip), "{ip} should be allowed");
        }
    }

    #[tokio::test]
    async fn rejects_non_https_and_literal_private_hosts() {
        assert!(validate_https_url("http://example.com").await.is_err());
        assert!(validate_https_url("https://localhost/x").await.is_err());
        assert!(validate_https_url("https://[::ffff:127.0.0.1]/x").await.is_err());
        assert!(validate_https_url("https://169.254.169.254/latest").await.is_err());
    }
}
