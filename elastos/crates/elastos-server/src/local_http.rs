use std::net::IpAddr;

use anyhow::{anyhow, Result};
use url::{Host, Url};

fn scheme_allowed(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

fn host_is_loopback(host: Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(addr) => IpAddr::V4(addr).is_loopback(),
        Host::Ipv6(addr) => IpAddr::V6(addr).is_loopback(),
    }
}

pub fn validate_loopback_http_url(raw: &str) -> Result<Url> {
    let parsed = Url::parse(raw).map_err(|e| anyhow!("invalid HTTP URL '{}': {}", raw, e))?;

    if !scheme_allowed(parsed.scheme()) {
        anyhow::bail!(
            "unsupported HTTP transport scheme '{}' for '{}'; only http:// or https:// are allowed",
            parsed.scheme(),
            raw
        );
    }

    let host = parsed
        .host()
        .ok_or_else(|| anyhow!("HTTP URL '{}' is missing a host", raw))?;
    if !host_is_loopback(host) {
        anyhow::bail!("non-loopback HTTP URL not allowed: {}", raw);
    }

    Ok(parsed)
}

#[derive(Debug, Clone)]
pub struct LoopbackHttpBaseUrl(Url);

impl LoopbackHttpBaseUrl {
    pub fn parse(raw: &str) -> Result<Self> {
        let mut parsed = validate_loopback_http_url(raw)?;
        if parsed.query().is_some() || parsed.fragment().is_some() {
            anyhow::bail!(
                "loopback HTTP base URL must not include query or fragment: {}",
                raw
            );
        }
        if !matches!(parsed.path(), "" | "/") {
            anyhow::bail!("loopback HTTP base URL must not include a path: {}", raw);
        }
        parsed.set_path("/");
        Ok(Self(parsed))
    }

    pub fn join(&self, relative: &str) -> Result<Url> {
        self.0
            .join(relative.trim_start_matches('/'))
            .map_err(|e| anyhow!("invalid loopback HTTP relative path '{}': {}", relative, e))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_loopback_http_url, LoopbackHttpBaseUrl};

    #[test]
    fn accepts_localhost() {
        assert!(validate_loopback_http_url("http://localhost:3000").is_ok());
    }

    #[test]
    fn accepts_loopback_ipv4() {
        assert!(validate_loopback_http_url("http://127.0.0.1:3000").is_ok());
    }

    #[test]
    fn accepts_loopback_ipv6() {
        assert!(validate_loopback_http_url("http://[::1]:3000").is_ok());
    }

    #[test]
    fn rejects_non_loopback_host() {
        let err = validate_loopback_http_url("https://example.com").unwrap_err();
        assert!(err
            .to_string()
            .contains("non-loopback HTTP URL not allowed"));
    }

    #[test]
    fn rejects_non_http_scheme() {
        let err = validate_loopback_http_url("unix:///tmp/elastos.sock").unwrap_err();
        assert!(err
            .to_string()
            .contains("unsupported HTTP transport scheme"));
    }

    #[test]
    fn base_url_accepts_root_only() {
        let base = LoopbackHttpBaseUrl::parse("http://127.0.0.1:3000").unwrap();
        assert_eq!(base.as_str(), "http://127.0.0.1:3000/");
        assert_eq!(
            base.join("/api/health").unwrap().as_str(),
            "http://127.0.0.1:3000/api/health"
        );
    }

    #[test]
    fn base_url_rejects_path_query_and_fragment() {
        assert!(LoopbackHttpBaseUrl::parse("http://127.0.0.1:3000/api").is_err());
        assert!(LoopbackHttpBaseUrl::parse("http://127.0.0.1:3000/?q=1").is_err());
        assert!(LoopbackHttpBaseUrl::parse("http://127.0.0.1:3000/#frag").is_err());
    }
}
