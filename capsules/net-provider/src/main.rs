//! ElastOS net-provider Capsule
//!
//! Defines the Browser/Net boundary. This first provider is intentionally
//! fail-closed: it validates Browser requests and refuses direct host networking
//! until a real Exit Provider is configured behind the Runtime.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::net::IpAddr;
use url::Url;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status {
        #[serde(default)]
        principal_id: Option<String>,
    },
    Resolve {
        hostname: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Connect {
        host: String,
        port: u16,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Stream {
        target: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        stream_nonce: Option<String>,
    },
    Http {
        #[serde(default)]
        schema: Option<String>,
        url: String,
        #[serde(default = "default_method")]
        method: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

struct NetProvider {
    exit_count: usize,
}

impl NetProvider {
    fn new() -> Self {
        Self { exit_count: 0 }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status { principal_id } => self.status(principal_id),
            Request::Resolve {
                hostname,
                principal_id,
                reason,
            } => self.resolve(&hostname, principal_id, reason),
            Request::Connect {
                host,
                port,
                principal_id,
                reason,
            } => self.connect(&host, port, principal_id, reason),
            Request::Stream {
                target,
                principal_id,
                reason,
                stream_nonce,
            } => self.stream(&target, principal_id, reason, stream_nonce),
            Request::Http {
                schema,
                url,
                method,
                principal_id,
                reason,
            } => self.http(schema, &url, &method, principal_id, reason),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        self.exit_count = configured_exit_count(&config);
        Response::ok(json!({
            "provider": "net-provider",
            "protocol_version": "1.0",
            "direct_network": false,
            "exit_count": self.exit_count,
        }))
    }

    fn status(&self, principal_id: Option<String>) -> Response {
        Response::ok(json!({
            "provider": "net-provider",
            "protocol_version": "1.0",
            "status": if self.exit_count == 0 { "fail_closed" } else { "exit_configured" },
            "principal_id": principal_id,
            "direct_network": false,
            "operations": ["resolve", "connect", "stream", "http"],
            "exit_count": self.exit_count,
        }))
    }

    fn resolve(
        &self,
        hostname: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        if let Err(err) = validate_public_host(hostname) {
            return Response::error("private_network_blocked", err);
        }
        self.exit_unavailable(
            "resolve",
            json!({
                "hostname": hostname,
                "principal_id": principal_id,
                "reason": reason,
            }),
        )
    }

    fn connect(
        &self,
        host: &str,
        port: u16,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        if let Err(err) = validate_public_host(host) {
            return Response::error("private_network_blocked", err);
        }
        if port == 0 {
            return Response::error("invalid_request", "connect requires a non-zero port");
        }
        self.exit_unavailable(
            "connect",
            json!({
                "host": host,
                "port": port,
                "principal_id": principal_id,
                "reason": reason,
            }),
        )
    }

    fn stream(
        &self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
        stream_nonce: Option<String>,
    ) -> Response {
        let Ok(url) = Url::parse(target) else {
            return Response::error("invalid_request", "stream target must be an absolute URL");
        };
        if !matches!(url.scheme(), "tcp" | "tls" | "https") {
            return Response::error(
                "invalid_request",
                "stream target must use tcp, tls, or https",
            );
        }
        let Some(host) = url.host_str() else {
            return Response::error("invalid_request", "stream target requires a host");
        };
        if let Err(err) = validate_public_host(host) {
            return Response::error("private_network_blocked", err);
        }
        self.exit_unavailable(
            "stream",
            json!({
                "target": target,
                "principal_id": principal_id,
                "reason": reason,
                "stream_nonce": stream_nonce,
            }),
        )
    }

    fn http(
        &self,
        schema: Option<String>,
        raw_url: &str,
        method: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        if schema
            .as_deref()
            .is_some_and(|value| value != "elastos.browser.net-request/v1")
        {
            return Response::error("invalid_request", "unsupported Browser/Net request schema");
        }
        let parsed = match Url::parse(raw_url) {
            Ok(url) => url,
            Err(err) => return Response::error("invalid_request", err.to_string()),
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return Response::error(
                "invalid_request",
                "http requests require http or https URLs",
            );
        }
        let Some(host) = parsed.host_str() else {
            return Response::error("invalid_request", "http request URL requires a host");
        };
        if let Err(err) = validate_public_host(host) {
            return Response::error("private_network_blocked", err);
        }
        if !matches!(method, "GET" | "HEAD") {
            return Response::error("invalid_request", "http request method must be GET or HEAD");
        }
        self.exit_unavailable(
            "http",
            json!({
                "url": parsed.as_str(),
                "method": method,
                "principal_id": principal_id,
                "reason": reason,
            }),
        )
    }

    fn exit_unavailable(&self, operation: &str, request: Value) -> Response {
        let _ = request;
        Response::error(
            "exit_unavailable",
            format!(
                "No Browser Exit provider is configured for {operation}; net-provider refuses direct host networking"
            ),
        )
    }
}

fn configured_exit_count(config: &Value) -> usize {
    config
        .get("extra")
        .and_then(|extra| extra.get("exits"))
        .or_else(|| config.get("exits"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn validate_public_host(host: &str) -> Result<(), String> {
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() {
        return Err("host must not be empty".to_string());
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\\' | b'\0'))
    {
        return Err(format!("invalid host: {host}"));
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        return Err(format!("private host blocked: {host}"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return validate_public_ip(ip).map_err(|_| format!("private IP blocked: {host}"));
    }
    Ok(())
}

fn validate_public_ip(ip: IpAddr) -> Result<(), ()> {
    match ip {
        IpAddr::V4(ip) => {
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
            {
                Err(())
            } else {
                Ok(())
            }
        }
        IpAddr::V6(ip) => {
            // Normalize IPv4-in-IPv6 forms FIRST: a dual-stack kernel routes
            // `::ffff:a.b.c.d` (IPv4-mapped) to the bare IPv4, so the v4
            // private/loopback/link-local guards MUST apply to the embedded
            // address. Without this, `::ffff:169.254.169.254` slips past every
            // v6 predicate and reaches cloud metadata / loopback (audit T3 —
            // confirmed end-to-end SSRF bypass).
            if let Some(v4) = ip.to_ipv4_mapped() {
                return validate_public_ip(IpAddr::V4(v4));
            }
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
            {
                Err(())
            } else if let Some(v4) = ip.to_ipv4() {
                // Deprecated IPv4-compatible (`::a.b.c.d`) and any other ::x form
                // that resolves to a v4 address. `::1`/`::` are already caught
                // above, so this only reaches real embedded v4 addresses — apply
                // the full v4 guard to them too, defensively.
                validate_public_ip(IpAddr::V4(v4))
            } else {
                Ok(())
            }
        }
    }
}

fn default_method() -> String {
    "GET".to_string()
}

fn main() {
    eprintln!(
        "net-provider: starting v{} (exit-provider required)",
        PROVIDER_VERSION
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = NetProvider::new();

    for line in stdin.lock().lines() {
        let response = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => match serde_json::from_str::<Request>(&line) {
                Ok(Request::Shutdown) => {
                    let response = Response::empty_ok();
                    let _ = write_response(&mut stdout, &response);
                    break;
                }
                Ok(request) => provider.handle(request),
                Err(err) => Response::error("invalid_request", err.to_string()),
            },
            Err(err) => Response::error("stdin_error", err.to_string()),
        };

        if write_response(&mut stdout, &response).is_err() {
            break;
        }
    }

    eprintln!("net-provider: exiting");
}

fn write_response(stdout: &mut io::Stdout, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit T3: IPv4-mapped IPv6 literals must NOT bypass the private-network
    /// guard. A dual-stack kernel routes `::ffff:169.254.169.254` to the bare
    /// IPv4, so the mapped forms must be refused exactly like their v4 forms.
    #[test]
    fn validate_public_ip_blocks_ipv4_mapped_private_targets() {
        for mapped in [
            "::ffff:169.254.169.254",
            "::ffff:127.0.0.1",
            "::ffff:192.168.1.1",
            "::ffff:10.0.0.5",
        ] {
            let ip: IpAddr = mapped.parse().expect("parse mapped v6");
            assert!(
                validate_public_ip(ip).is_err(),
                "{mapped} must be blocked as a private/loopback/link-local target"
            );
        }
        assert!(validate_public_ip("2606:4700:4700::1111".parse().unwrap()).is_ok());
        assert!(validate_public_ip("::ffff:1.1.1.1".parse().unwrap()).is_ok());
    }

    fn status_text(response: Response) -> String {
        serde_json::to_value(response).unwrap()["status"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn error_code(response: Response) -> String {
        serde_json::to_value(response).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn status_is_fail_closed_without_exits() {
        let provider = NetProvider::new();
        let response =
            serde_json::to_value(provider.status(Some("person:local:test".to_string()))).unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["status"], "fail_closed");
        assert_eq!(response["data"]["direct_network"], false);
    }

    #[test]
    fn http_public_url_fails_closed_until_exit_exists() {
        let provider = NetProvider::new();
        let response = provider.http(
            Some("elastos.browser.net-request/v1".to_string()),
            "https://glidefinance.io/",
            "GET",
            Some("person:local:test".to_string()),
            Some("open browser address".to_string()),
        );
        assert_eq!(status_text(response), "error");
    }

    #[test]
    fn http_blocks_private_hosts() {
        let provider = NetProvider::new();
        for url in [
            "http://localhost/",
            "http://127.0.0.1/",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://router.local/",
        ] {
            let response = provider.http(
                Some("elastos.browser.net-request/v1".to_string()),
                url,
                "GET",
                None,
                None,
            );
            assert_eq!(error_code(response), "private_network_blocked");
        }
    }

    #[test]
    fn http_rejects_unsupported_methods_and_schemes() {
        let provider = NetProvider::new();
        assert_eq!(
            error_code(provider.http(
                Some("elastos.browser.net-request/v1".to_string()),
                "https://glidefinance.io/",
                "POST",
                None,
                None
            )),
            "invalid_request"
        );
        assert_eq!(
            error_code(provider.http(
                Some("elastos.browser.net-request/v1".to_string()),
                "ftp://example.com/",
                "GET",
                None,
                None
            )),
            "invalid_request"
        );
    }

    #[test]
    fn request_decode_rejects_hidden_authority_fields() {
        let err = serde_json::from_value::<Request>(json!({
            "op": "http",
            "schema": "elastos.browser.net-request/v1",
            "url": "https://glidefinance.io/",
            "method": "GET",
            "raw_socket": true
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }
}
