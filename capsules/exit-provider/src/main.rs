//! ElastOS exit-provider Capsule
//!
//! Internal contract behind `net-provider`. This first implementation is
//! deliberately fail-closed: it validates exit requests and refuses egress until
//! an operator configures a real local, Carrier-routed, privacy, paid, or
//! enterprise exit backend.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use std::net::IpAddr;
use std::time::Duration;
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
    Quote {
        target: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    OpenStream {
        target: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    CloseStream {
        stream_id: String,
        #[serde(default)]
        principal_id: Option<String>,
    },
    HttpFetch {
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

struct ExitProvider {
    backends: Vec<ExitBackendConfig>,
    agent: ureq::Agent,
}

impl ExitProvider {
    fn new() -> Self {
        Self {
            backends: Vec::new(),
            agent: http_agent(default_timeout_secs()),
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status { principal_id } => self.status(principal_id),
            Request::Quote {
                target,
                principal_id,
                reason,
            } => self.quote(&target, principal_id, reason),
            Request::OpenStream {
                target,
                principal_id,
                reason,
            } => self.open_stream(&target, principal_id, reason),
            Request::CloseStream {
                stream_id,
                principal_id,
            } => self.close_stream(&stream_id, principal_id),
            Request::HttpFetch {
                url,
                method,
                principal_id,
                reason,
            } => self.http_fetch(&url, &method, principal_id, reason),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let config = match parse_config(config) {
            Ok(config) => config,
            Err(err) => return Response::error("invalid_config", err),
        };
        self.agent = http_agent(config.timeout_secs);
        self.backends = config.backends;
        Response::ok(json!({
            "provider": "exit-provider",
            "protocol_version": "1.0",
            "backend_count": self.backends.len(),
            "direct_network": false,
        }))
    }

    fn status(&self, principal_id: Option<String>) -> Response {
        Response::ok(json!({
            "provider": "exit-provider",
            "protocol_version": "1.0",
            "status": if self.backends.is_empty() { "fail_closed" } else { "backend_configured" },
            "principal_id": principal_id,
            "backend_count": self.backends.len(),
            "direct_network": false,
            "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
        }))
    }

    fn quote(
        &self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        let parsed = match validate_target(target) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        if let Some(host) = parsed.host_str() {
            if let Err(err) = validate_public_host(host) {
                return Response::error("private_network_blocked", err);
            }
        }
        self.exit_unavailable(
            "quote",
            json!({
                "target": target,
                "principal_id": principal_id,
                "reason": reason,
            }),
        )
    }

    fn open_stream(
        &self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        let parsed = match validate_target(target) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        let Some(host) = parsed.host_str() else {
            return Response::error("invalid_request", "stream target requires a host");
        };
        let backend = self.backend_for_stream(&parsed);
        if let Err(err) = validate_public_host(host) {
            if !backend.is_some_and(|backend| backend.allow_private_targets) {
                return Response::error("private_network_blocked", err);
            }
        }
        let Some(backend) = backend else {
            if self.backends.is_empty() {
                return self.exit_unavailable(
                    "open_stream",
                    json!({
                        "target": target,
                        "principal_id": principal_id,
                        "reason": reason,
                    }),
                );
            }
            return Response::error(
                "exit_policy_blocked",
                format!("No Browser Exit backend allows host {host}; exit-provider refuses direct host networking"),
            );
        };
        let stream_id = format!(
            "stream:{}:{}",
            backend.id,
            stable_stream_suffix(parsed.as_str(), principal_id.as_deref())
        );
        let adapter_ipc = backend.adapter_ipc.as_ref().map(|ipc| {
            json!({
                "schema": "elastos.adapter-ipc/v1",
                "kind": ipc.kind,
                "path": ipc.path,
                "stream_id": stream_id,
            })
        });
        let relay_ipc = backend.relay_ipc.as_ref().map(|ipc| {
            json!({
                "schema": "elastos.exit.relay-ipc/v1",
                "kind": ipc.kind,
                "path": ipc.path,
                "stream_id": stream_id,
            })
        });
        Response::ok(json!({
            "schema": "elastos.exit.stream-session/v1",
            "backend": backend.id,
            "stream_id": stream_id,
            "target": parsed.as_str(),
            "scheme": parsed.scheme(),
            "host": host,
            "principal_id": principal_id,
            "reason": reason,
            "engine_owns_tls": matches!(parsed.scheme(), "tls" | "https"),
            "state": "reserved",
            "byte_transport": if adapter_ipc.is_some() { "adapter_ipc" } else { "not_attached" },
            "adapter_ipc": adapter_ipc,
            "relay_ipc": relay_ipc
        }))
    }

    fn close_stream(&self, stream_id: &str, principal_id: Option<String>) -> Response {
        if !is_safe_id(stream_id) {
            return Response::error("invalid_request", "close_stream requires a safe stream_id");
        }
        Response::ok(json!({
            "closed": false,
            "stream_id": stream_id,
            "principal_id": principal_id,
            "reason": "no exit backend is configured"
        }))
    }

    fn http_fetch(
        &self,
        raw_url: &str,
        method: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        if !matches!(method, "GET" | "HEAD") {
            return Response::error("invalid_request", "http_fetch method must be GET or HEAD");
        }
        let parsed = match validate_http_fetch_url(raw_url) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        let Some(host) = parsed.host_str() else {
            return Response::error("invalid_request", "http_fetch URL requires a host");
        };
        let backend = self.backend_for_http_fetch(&parsed);
        if let Err(err) = validate_public_host(host) {
            if !backend.is_some_and(|backend| backend.allow_private_targets) {
                return Response::error("private_network_blocked", err);
            }
        }
        let Some(backend) = backend else {
            return self.exit_unavailable(
                "http_fetch",
                json!({
                    "url": raw_url,
                    "method": method,
                    "principal_id": principal_id,
                    "reason": reason,
                }),
            );
        };
        self.http_fetch_with_backend(backend, parsed, method, principal_id, reason)
    }

    fn exit_unavailable(&self, operation: &str, request: Value) -> Response {
        let _ = request;
        Response::error(
            "exit_unavailable",
            format!(
                "No Browser Exit backend is configured for {operation}; exit-provider refuses direct host networking"
            ),
        )
    }

    fn backend_for_http_fetch(&self, target: &Url) -> Option<&ExitBackendConfig> {
        self.backends.iter().find(|backend| {
            backend.kind == ExitBackendKind::HttpFetch && backend.allows_target(target)
        })
    }

    fn backend_for_stream(&self, target: &Url) -> Option<&ExitBackendConfig> {
        self.backends.iter().find(|backend| {
            backend.kind == ExitBackendKind::StreamRelay && backend.allows_target(target)
        })
    }

    fn http_fetch_with_backend(
        &self,
        backend: &ExitBackendConfig,
        url: Url,
        method: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        let request = match method {
            "HEAD" => self.agent.head(url.as_str()),
            _ => self.agent.get(url.as_str()),
        }
        .set("User-Agent", "ElastOS-exit-provider/0.1");

        let response = match request.call() {
            Ok(response) => response,
            Err(err) => return Response::error("backend_error", err.to_string()),
        };
        let status_code = response.status();
        let content_type = response.header("content-type").map(str::to_string);
        let mut body = Vec::new();
        let mut truncated = false;
        if method != "HEAD" {
            let limit = backend.max_body_bytes.saturating_add(1) as u64;
            if let Err(err) = response.into_reader().take(limit).read_to_end(&mut body) {
                return Response::error("backend_error", err.to_string());
            }
            if body.len() > backend.max_body_bytes {
                body.truncate(backend.max_body_bytes);
                truncated = true;
            }
        }
        Response::ok(json!({
            "schema": "elastos.exit.http-fetch.result/v1",
            "backend": backend.id,
            "url": url.as_str(),
            "method": method,
            "principal_id": principal_id,
            "reason": reason,
            "status_code": status_code,
            "content_type": content_type,
            "body_bytes": body.len(),
            "body_truncated": truncated,
            "body_text": String::from_utf8_lossy(&body),
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitConfig {
    #[serde(default)]
    backends: Vec<ExitBackendConfig>,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitBackendConfig {
    id: String,
    kind: ExitBackendKind,
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_schemes: Vec<String>,
    #[serde(default)]
    allowed_ports: Vec<u16>,
    #[serde(default)]
    allow_private_targets: bool,
    #[serde(default = "default_max_body_bytes")]
    max_body_bytes: usize,
    #[serde(default)]
    adapter_ipc: Option<AdapterIpcConfig>,
    #[serde(default)]
    relay_ipc: Option<RelayIpcConfig>,
}

impl ExitBackendConfig {
    fn allows_target(&self, target: &Url) -> bool {
        let Some(host) = target.host_str() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let host_allowed = self.allowed_hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            if allowed == "*" {
                return true;
            }
            if let Some(suffix) = allowed.strip_prefix("*.") {
                host.ends_with(&format!(".{suffix}"))
            } else {
                host == allowed
            }
        });
        host_allowed
            && self.allows_scheme(target.scheme())
            && self.allows_port(target.port_or_known_default())
    }

    fn allows_scheme(&self, scheme: &str) -> bool {
        if self.allowed_schemes.is_empty() {
            return match self.kind {
                ExitBackendKind::StreamRelay => matches!(scheme, "tcp" | "tls"),
                ExitBackendKind::HttpFetch => matches!(scheme, "http" | "https"),
            };
        }
        self.allowed_schemes.iter().any(|allowed| allowed == scheme)
    }

    fn allows_port(&self, port: Option<u16>) -> bool {
        let Some(port) = port else {
            return false;
        };
        self.allowed_ports.is_empty() || self.allowed_ports.contains(&port)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExitBackendKind {
    HttpFetch,
    StreamRelay,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdapterIpcConfig {
    kind: AdapterIpcKind,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelayIpcConfig {
    kind: RelayIpcKind,
    path: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterIpcKind {
    UnixSocket,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RelayIpcKind {
    UnixSocket,
}

fn parse_config(config: Value) -> Result<ExitConfig, String> {
    let config = match config.get("extra") {
        Some(extra) if extra.is_null() => json!({}),
        Some(extra) => extra.clone(),
        None if looks_like_bridge_provider_config(&config) => json!({}),
        None => config,
    };
    let config = serde_json::from_value::<ExitConfig>(config).map_err(|err| err.to_string())?;
    if config.timeout_secs == 0 || config.timeout_secs > 60 {
        return Err("exit-provider timeout_secs must be between 1 and 60".to_string());
    }
    for backend in &config.backends {
        validate_backend(backend)?;
    }
    Ok(config)
}

fn looks_like_bridge_provider_config(config: &Value) -> bool {
    config.get("base_path").is_some()
        || config.get("allowed_paths").is_some()
        || config.get("read_only").is_some()
        || config.get("encryption_key").is_some()
}

fn validate_backend(backend: &ExitBackendConfig) -> Result<(), String> {
    if !is_safe_id(&backend.id) {
        return Err("exit backend id must be a safe identifier".to_string());
    }
    if backend.allowed_hosts.is_empty() {
        return Err(format!(
            "exit backend '{}' must declare at least one allowed host",
            backend.id
        ));
    }
    if backend.max_body_bytes == 0 || backend.max_body_bytes > 1024 * 1024 {
        return Err(format!(
            "exit backend '{}' max_body_bytes must be between 1 and 1048576",
            backend.id
        ));
    }
    if let Some(adapter_ipc) = &backend.adapter_ipc {
        if backend.kind != ExitBackendKind::StreamRelay {
            return Err(format!(
                "exit backend '{}' adapter_ipc is only valid for stream_relay backends",
                backend.id
            ));
        }
        validate_adapter_ipc(adapter_ipc)?;
    }
    if let Some(relay_ipc) = &backend.relay_ipc {
        if backend.kind != ExitBackendKind::StreamRelay {
            return Err(format!(
                "exit backend '{}' relay_ipc is only valid for stream_relay backends",
                backend.id
            ));
        }
        if backend.adapter_ipc.is_none() {
            return Err(format!(
                "exit backend '{}' relay_ipc requires adapter_ipc",
                backend.id
            ));
        }
        validate_relay_ipc(relay_ipc)?;
    }
    if let (Some(adapter_ipc), Some(relay_ipc)) = (&backend.adapter_ipc, &backend.relay_ipc) {
        if adapter_ipc.path == relay_ipc.path {
            return Err(format!(
                "exit backend '{}' adapter_ipc and relay_ipc paths must differ",
                backend.id
            ));
        }
    }
    for host in &backend.allowed_hosts {
        validate_allowed_host(host)?;
    }
    for scheme in &backend.allowed_schemes {
        validate_allowed_scheme(backend.kind, scheme)?;
    }
    for port in &backend.allowed_ports {
        if *port == 0 {
            return Err(format!(
                "exit backend '{}' allowed_ports must contain TCP ports between 1 and 65535",
                backend.id
            ));
        }
    }
    Ok(())
}

fn validate_allowed_scheme(kind: ExitBackendKind, scheme: &str) -> Result<(), String> {
    let valid = match kind {
        ExitBackendKind::StreamRelay => matches!(scheme, "tcp" | "tls"),
        ExitBackendKind::HttpFetch => matches!(scheme, "http" | "https"),
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{kind:?} backend allowed_schemes may not contain '{scheme}'"
        ))
    }
}

fn validate_adapter_ipc(adapter_ipc: &AdapterIpcConfig) -> Result<(), String> {
    validate_ipc_path("adapter_ipc", &adapter_ipc.path)
}

fn validate_relay_ipc(relay_ipc: &RelayIpcConfig) -> Result<(), String> {
    validate_ipc_path("relay_ipc", &relay_ipc.path)
}

fn validate_ipc_path(label: &str, path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{label} path must not be empty"));
    }
    if !path.starts_with('/') {
        return Err(format!("{label} path must be absolute"));
    }
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(format!("{label} path must not contain whitespace or NUL"));
    }
    Ok(())
}

fn validate_allowed_host(host: &str) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("allowed host must not be empty".to_string());
    }
    if host == "*" {
        return Ok(());
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    validate_public_host_shape(host)
}

fn validate_target(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|err| err.to_string())?;
    if !matches!(parsed.scheme(), "tcp" | "tls" | "http" | "https") {
        return Err("exit target must use tcp, tls, http, or https".to_string());
    }
    let Some(host) = parsed.host_str() else {
        return Err("exit target requires a host".to_string());
    };
    validate_public_host_shape(host)?;
    Ok(parsed)
}

fn validate_http_fetch_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|err| err.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("http_fetch URL must use http or https".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("http_fetch URL must not contain credentials".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("http_fetch URL requires a host".to_string());
    }
    Ok(parsed)
}

fn validate_public_host(host: &str) -> Result<(), String> {
    let host = host.trim().trim_matches(['[', ']']);
    validate_public_host_shape(host)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return validate_public_ip(ip).map_err(|_| format!("private IP blocked: {host}"));
    }
    Ok(())
}

fn validate_public_host_shape(host: &str) -> Result<(), String> {
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
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
            {
                Err(())
            } else {
                Ok(())
            }
        }
    }
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

fn stable_stream_suffix(target: &str, principal_id: Option<&str>) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in target.bytes().chain(principal_id.unwrap_or("").bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_timeout_secs() -> u64 {
    10
}

fn default_max_body_bytes() -> usize {
    64 * 1024
}

fn http_agent(timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
}

fn main() {
    eprintln!(
        "exit-provider: starting v{} (backend required)",
        PROVIDER_VERSION
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = ExitProvider::new();

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

    eprintln!("exit-provider: exiting");
}

fn write_response(stdout: &mut io::Stdout, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn error_code(response: Response) -> String {
        serde_json::to_value(response).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn status_is_fail_closed_without_backend() {
        let provider = ExitProvider::new();
        let response =
            serde_json::to_value(provider.status(Some("person:local:test".to_string()))).unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["status"], "fail_closed");
        assert_eq!(response["data"]["direct_network"], false);
    }

    #[test]
    fn provider_bridge_default_config_initializes_empty() {
        let mut provider = ExitProvider::new();
        let response = serde_json::to_value(provider.init(json!({
            "base_path": "",
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": ""
        })))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["backend_count"], 0);
    }

    #[test]
    fn public_stream_target_fails_closed_until_backend_exists() {
        let provider = ExitProvider::new();
        let response = provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        );
        assert_eq!(error_code(response), "exit_unavailable");
    }

    #[test]
    fn configured_stream_backend_returns_reserved_session_receipt() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["schema"], "elastos.exit.stream-session/v1");
        assert_eq!(response["data"]["backend"], "stream-proof");
        assert_eq!(response["data"]["target"], "tls://glidefinance.io:443");
        assert_eq!(response["data"]["engine_owns_tls"], true);
        assert_eq!(response["data"]["state"], "reserved");
        assert_eq!(response["data"]["byte_transport"], "not_attached");
        assert_eq!(response["data"]["adapter_ipc"], serde_json::Value::Null);
        assert_eq!(response["data"]["relay_ipc"], serde_json::Value::Null);
    }

    #[test]
    fn configured_stream_backend_can_return_adapter_ipc_descriptor() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["byte_transport"], "adapter_ipc");
        assert_eq!(
            response["data"]["adapter_ipc"]["schema"],
            "elastos.adapter-ipc/v1"
        );
        assert_eq!(response["data"]["adapter_ipc"]["kind"], "unix_socket");
        assert_eq!(
            response["data"]["adapter_ipc"]["path"],
            "/tmp/elastos-browser-stream.sock"
        );
        assert_eq!(
            response["data"]["adapter_ipc"]["stream_id"],
            response["data"]["stream_id"]
        );
        assert_eq!(response["data"]["relay_ipc"], serde_json::Value::Null);
    }

    #[test]
    fn configured_stream_backend_can_return_exit_relay_ipc_descriptor() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                },
                "relay_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-exit-relay.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["relay_ipc"]["schema"],
            "elastos.exit.relay-ipc/v1"
        );
        assert_eq!(response["data"]["relay_ipc"]["kind"], "unix_socket");
        assert_eq!(
            response["data"]["relay_ipc"]["path"],
            "/tmp/elastos-exit-relay.sock"
        );
        assert_eq!(
            response["data"]["relay_ipc"]["stream_id"],
            response["data"]["stream_id"]
        );
    }

    #[test]
    fn http_fetch_blocks_private_targets() {
        let provider = ExitProvider::new();
        for url in [
            "http://localhost/",
            "http://127.0.0.1/",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://router.local/",
        ] {
            assert_eq!(
                error_code(provider.http_fetch(url, "GET", None, None)),
                "private_network_blocked"
            );
        }
    }

    #[test]
    fn request_decode_rejects_hidden_network_authority_fields() {
        let err = serde_json::from_value::<Request>(json!({
            "op": "open_stream",
            "target": "tls://glidefinance.io:443",
            "raw_socket": true
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }

    #[test]
    fn backend_config_rejects_invalid_adapter_ipc() {
        let mut provider = ExitProvider::new();
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-http-ipc",
                    "kind": "http_fetch",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-http-relay",
                    "kind": "http_fetch",
                    "allowed_hosts": ["glidefinance.io"],
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-exit-relay.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-relay-without-adapter",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-exit-relay.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-shared-ipc",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    },
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-relative-ipc",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "relative.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
    }

    #[test]
    fn configured_http_fetch_backend_can_fetch_allowlisted_target() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nok",
                )
                .unwrap();
        });

        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "local-test",
                "kind": "http_fetch",
                "allowed_hosts": ["127.0.0.1"],
                "allow_private_targets": true,
                "max_body_bytes": 16
            }],
            "timeout_secs": 2
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.http_fetch(
            &format!("http://{addr}/"),
            "GET",
            Some("person:local:test".to_string()),
            Some("test controlled exit".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["backend"], "local-test");
        assert_eq!(response["data"]["status_code"], 200);
        assert_eq!(response["data"]["body_text"], "ok");
    }

    #[test]
    fn configured_http_fetch_backend_rejects_unallowlisted_target() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "only-example",
                "kind": "http_fetch",
                "allowed_hosts": ["example.com"]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        assert_eq!(
            error_code(provider.http_fetch("https://glidefinance.io/", "GET", None, None)),
            "exit_unavailable"
        );
    }

    #[test]
    fn wildcard_stream_backend_allows_public_hosts_but_not_private_targets() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "public-web",
                "kind": "stream_relay",
                "allowed_hosts": ["*"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let public = serde_json::to_value(provider.open_stream(
            "tls://whatismyip.com:443",
            Some("person:local:test".to_string()),
            Some("check exit IP".to_string()),
        ))
        .unwrap();
        assert_eq!(public["status"], "ok");
        assert_eq!(public["data"]["target"], "tls://whatismyip.com:443");

        assert_eq!(
            error_code(provider.open_stream("tcp://127.0.0.1:80", None, None)),
            "private_network_blocked"
        );
    }

    #[test]
    fn stream_backend_can_limit_schemes_and_ports() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "https-only",
                "kind": "stream_relay",
                "allowed_hosts": ["example.com"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let allowed = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:test".to_string()),
            Some("test constrained exit".to_string()),
        ))
        .unwrap();
        assert_eq!(allowed["status"], "ok");

        assert_eq!(
            error_code(provider.open_stream("tcp://example.com:443", None, None)),
            "exit_policy_blocked"
        );
        assert_eq!(
            error_code(provider.open_stream("tls://example.com:8443", None, None)),
            "exit_policy_blocked"
        );
    }

    #[test]
    fn backend_config_rejects_hidden_fields() {
        let mut provider = ExitProvider::new();
        let response = provider.init(json!({
            "backends": [{
                "id": "bad",
                "kind": "http_fetch",
                "allowed_hosts": ["example.com"],
                "raw_socket": true
            }]
        }));

        assert_eq!(error_code(response), "invalid_config");
    }
}
