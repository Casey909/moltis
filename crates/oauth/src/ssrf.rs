//! Lightweight SSRF protection for OAuth discovery endpoints.
//!
//! Resolves the URL host to IP addresses and rejects requests targeting
//! private, loopback, link-local, or otherwise non-routable ranges.

use std::net::IpAddr;

use {tokio::net::lookup_host, url::Url};

use crate::{Error, Result};

/// Check if an IP address is private, loopback, link-local, or otherwise
/// unsuitable for outbound fetches.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 100.64.0.0/10 (CGNAT)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
                // 192.0.0.0/24
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0)
        },
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xFE00) == 0xFC00
                // fe80::/10 (link-local)
                || (v6.segments()[0] & 0xFFC0) == 0xFE80
        },
    }
}

/// Check if an IP is covered by an SSRF allowlist entry.
fn is_ssrf_allowed(ip: &IpAddr, allowlist: &[ipnet::IpNet]) -> bool {
    allowlist.iter().any(|net| net.contains(ip))
}

/// SSRF protection for OAuth discovery endpoints.
///
/// Resolves the URL host and rejects private/loopback/link-local IPs unless
/// explicitly allowlisted.
pub async fn ssrf_check(url: &Url, allowlist: &[ipnet::IpNet]) -> Result<()> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::message("URL has no host"))?;

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) && !is_ssrf_allowed(&ip, allowlist) {
            return Err(Error::message(format!(
                "SSRF blocked: {host} resolves to private IP {ip}"
            )));
        }
        return Ok(());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| Error::message(format!("DNS resolution failed for {host}: {e}")))?
        .map(|socket_addr| socket_addr.ip())
        .collect();

    if addrs.is_empty() {
        return Err(Error::message(format!(
            "DNS resolution failed for {host}"
        )));
    }

    for ip in &addrs {
        if is_private_ip(ip) && !is_ssrf_allowed(ip, allowlist) {
            return Err(Error::message(format!(
                "SSRF blocked: {host} resolves to private IP {ip}"
            )));
        }
    }

    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ipv4_ranges() {
        let cases = [
            ("127.0.0.1", true),
            ("192.168.1.1", true),
            ("10.0.0.1", true),
            ("172.16.0.1", true),
            ("169.254.1.1", true),
            ("0.0.0.0", true),
            ("100.64.0.1", true),
            ("8.8.8.8", false),
            ("1.1.1.1", false),
        ];
        for (addr, expected) in cases {
            let ip: IpAddr = addr.parse().unwrap();
            assert_eq!(is_private_ip(&ip), expected, "{addr}");
        }
    }

    #[test]
    fn private_ipv6_ranges() {
        let cases = [
            ("::1", true),
            ("::", true),
            ("fd00::1", true),
            ("fe80::1", true),
            ("2607:f8b0:4004:800::200e", false),
        ];
        for (addr, expected) in cases {
            let ip: IpAddr = addr.parse().unwrap();
            assert_eq!(is_private_ip(&ip), expected, "{addr}");
        }
    }

    #[tokio::test]
    async fn blocks_localhost() {
        let url = Url::parse("http://127.0.0.1/secret").unwrap();
        let result = ssrf_check(&url, &[]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SSRF"));
    }

    #[tokio::test]
    async fn blocks_private_network() {
        let url = Url::parse("http://192.168.1.1/admin").unwrap();
        let result = ssrf_check(&url, &[]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SSRF"));
    }

    #[tokio::test]
    async fn allowlist_permits_private() {
        let allowlist: Vec<ipnet::IpNet> = vec!["192.168.1.0/24".parse().unwrap()];
        let url = Url::parse("http://192.168.1.1/api").unwrap();
        let result = ssrf_check(&url, &allowlist).await;
        assert!(result.is_ok());
    }
}
