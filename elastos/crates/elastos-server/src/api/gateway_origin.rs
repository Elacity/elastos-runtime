use super::*;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::api::gateway) struct EffectiveGatewayOrigin {
    authority: String,
    host: String,
    origin: String,
    secure: bool,
}

impl EffectiveGatewayOrigin {
    pub(in crate::api::gateway) fn authority(&self) -> &str {
        &self.authority
    }

    pub(in crate::api::gateway) fn origin(&self) -> &str {
        &self.origin
    }

    pub(in crate::api::gateway) fn secure(&self) -> bool {
        self.secure
    }
}

pub(in crate::api::gateway) fn effective_gateway_origin(
    headers: &HeaderMap,
) -> anyhow::Result<EffectiveGatewayOrigin> {
    let authority = validated_host_authority(headers)?;
    let host = authority_host(&authority)?;
    let loopback = gateway_host_is_loopback(&host);
    let explicit_origin = request_origin_candidate(headers)?;
    let (origin, secure) = match explicit_origin {
        Some((origin, allow_path)) => {
            let parsed = parse_request_origin(&origin, loopback, allow_path)?;
            if parsed.authority() != authority {
                anyhow::bail!("request origin authority must match Host");
            }
            (parsed.origin().to_string(), parsed.secure())
        }
        None => {
            let scheme = if loopback { "http" } else { "https" };
            (format!("{scheme}://{authority}"), scheme == "https")
        }
    };
    Ok(EffectiveGatewayOrigin {
        authority,
        host,
        origin,
        secure,
    })
}

pub(in crate::api::gateway) fn validated_gateway_host(
    headers: &HeaderMap,
) -> anyhow::Result<String> {
    let authority = validated_host_authority(headers)?;
    authority_host(&authority)
}

fn request_origin_candidate(headers: &HeaderMap) -> anyhow::Result<Option<(String, bool)>> {
    if let Some(origin) = header_value(headers, "origin")? {
        if origin.eq_ignore_ascii_case("null") {
            return Ok(None);
        }
        return Ok(Some((origin.to_string(), false)));
    }
    if let Some(referer) = header_value(headers, "referer")? {
        return Ok(Some((referer.to_string(), true)));
    }
    Ok(None)
}

fn validated_host_authority(headers: &HeaderMap) -> anyhow::Result<String> {
    let raw = header_value(headers, "host")?
        .ok_or_else(|| anyhow::anyhow!("request Host header is missing"))?;
    normalize_authority(raw)
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> anyhow::Result<Option<&'a str>> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| anyhow::anyhow!("request {name} header is invalid"))?;
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    Ok(Some(value))
}

fn parse_request_origin(
    input: &str,
    loopback_host: bool,
    allow_path: bool,
) -> anyhow::Result<EffectiveGatewayOrigin> {
    let url = Url::parse(input).map_err(|_| anyhow::anyhow!("request origin is invalid"))?;
    if !allow_path && url.path() != "/" && !url.path().is_empty() {
        anyhow::bail!("request origin must not include a path");
    }
    let authority = normalize_authority(url.authority())?;
    let host = authority_host(&authority)?;
    let secure = match url.scheme() {
        "https" => true,
        "http" if loopback_host => false,
        "http" => anyhow::bail!("non-loopback request origin must use https"),
        _ => anyhow::bail!("request origin must use http or https"),
    };
    Ok(EffectiveGatewayOrigin {
        authority,
        host,
        origin: url.origin().ascii_serialization(),
        secure,
    })
}

fn authority_host(authority: &str) -> anyhow::Result<String> {
    let url = Url::parse(&format!("http://{authority}/"))
        .map_err(|_| anyhow::anyhow!("request Host header is invalid"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("request Host header is invalid"))?;
    Ok(host.trim_end_matches('.').to_ascii_lowercase())
}

fn normalize_authority(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty()
        || value.contains('/')
        || value.contains('@')
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        anyhow::bail!("request Host header is invalid");
    }
    let url = Url::parse(&format!("http://{value}/"))
        .map_err(|_| anyhow::anyhow!("request Host header is invalid"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("request Host header is invalid"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Ok(authority)
}

fn gateway_host_is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || matches!(host, "127.0.0.1" | "0.0.0.0" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn derives_http_loopback_origin_from_matching_origin() {
        let headers = headers(&[
            ("host", "localhost:61180"),
            ("origin", "http://localhost:61180"),
        ]);
        let origin = effective_gateway_origin(&headers).unwrap();
        assert_eq!(origin.origin(), "http://localhost:61180");
        assert_eq!(origin.authority(), "localhost:61180");
        assert!(!origin.secure());
    }

    #[test]
    fn derives_https_domain_origin_with_port() {
        let headers = headers(&[
            ("host", "chat.example.test:7443"),
            ("origin", "https://chat.example.test:7443"),
        ]);
        let origin = effective_gateway_origin(&headers).unwrap();
        assert_eq!(origin.origin(), "https://chat.example.test:7443");
        assert_eq!(origin.authority(), "chat.example.test:7443");
        assert!(origin.secure());
    }

    #[test]
    fn opaque_origin_uses_validated_host() {
        let headers = headers(&[("host", "chat.example.test:9443"), ("origin", "null")]);
        let origin = effective_gateway_origin(&headers).unwrap();
        assert_eq!(origin.origin(), "https://chat.example.test:9443");
        assert!(origin.secure());
    }

    #[test]
    fn mismatched_origin_and_host_are_rejected() {
        let headers = headers(&[
            ("host", "chat.example.test"),
            ("origin", "https://evil.example.test"),
        ]);
        let err = effective_gateway_origin(&headers).unwrap_err().to_string();
        assert!(err.contains("must match Host"));
    }

    #[test]
    fn spoofed_forwarded_headers_are_ignored() {
        let headers = headers(&[
            ("host", "localhost:61180"),
            ("x-forwarded-host", "chat.example.test"),
            ("x-forwarded-proto", "https"),
        ]);
        let origin = effective_gateway_origin(&headers).unwrap();
        assert_eq!(origin.origin(), "http://localhost:61180");
        assert!(!origin.secure());
    }

    #[test]
    fn referer_can_supply_matching_origin() {
        let headers = headers(&[
            ("host", "chat.example.test:8443"),
            ("referer", "https://chat.example.test:8443/apps/home/"),
        ]);
        let origin = effective_gateway_origin(&headers).unwrap();
        assert_eq!(origin.origin(), "https://chat.example.test:8443");
        assert!(origin.secure());
    }
}
