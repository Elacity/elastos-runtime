//! ElastOS availability-provider Capsule
//!
//! Bridges the runtime content provider to explicitly configured SmartWeb
//! availability targets. App capsules never see Elacity, IPFS Cluster, or
//! supernode APIs directly.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::time::Duration;

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
    Ensure(EnsureRequest),
    Status,
    Shutdown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EnsureRequest {
    cid: String,
    uri: String,
    policy: String,
    #[serde(default)]
    local: Value,
    #[serde(default)]
    object_did: Option<String>,
    #[serde(default)]
    publisher_did: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AvailabilityConfig {
    targets: Vec<AvailabilityTarget>,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AvailabilityTarget {
    id: String,
    ensure_url: String,
    #[serde(default)]
    authorization: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
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
        Response::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Response::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

struct AvailabilityProvider {
    targets: Vec<AvailabilityTarget>,
    agent: ureq::Agent,
}

impl AvailabilityProvider {
    fn new() -> Self {
        Self {
            targets: Vec::new(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(default_timeout_secs()))
                .build(),
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Ensure(request) => self.ensure(request),
            Request::Status => self.status(),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let config = match parse_config(config) {
            Ok(config) => config,
            Err(err) => return Response::error("invalid_config", err),
        };
        self.agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build();
        self.targets = config.targets;
        Response::ok(json!({
            "provider": "availability-provider",
            "protocol_version": "1.0",
            "target_count": self.targets.len(),
        }))
    }

    fn ensure(&self, request: EnsureRequest) -> Response {
        if request.cid.trim().is_empty() {
            return Response::error("invalid_request", "ensure requires cid");
        }
        if self.targets.is_empty() {
            return Response::ok(json!({
                "availability": repair_needed("availability-provider", &request, "no availability targets configured")
            }));
        }

        let mut last_error = None;
        for target in &self.targets {
            match self.ensure_target(target, &request) {
                Ok(availability) => {
                    return Response::ok(json!({ "availability": availability }));
                }
                Err(err) => last_error = Some(format!("{}: {}", target.id, err)),
            }
        }

        Response::ok(json!({
            "availability": repair_needed(
                &self.targets[0].id,
                &request,
                last_error.unwrap_or_else(|| "availability target failed".to_string()),
            )
        }))
    }

    fn ensure_target(
        &self,
        target: &AvailabilityTarget,
        request: &EnsureRequest,
    ) -> Result<Value, String> {
        let mut http = self
            .agent
            .post(&target.ensure_url)
            .set("Content-Type", "application/json");
        if let Some(value) = &target.authorization {
            http = http.set("Authorization", value);
        }
        for (name, value) in &target.headers {
            http = http.set(name, value);
        }

        let response = http
            .send_json(json!(request))
            .map_err(upstream_error_message)?;
        let value = response
            .into_json::<Value>()
            .map_err(|err| format!("invalid JSON response: {err}"))?;
        normalize_upstream_availability(target, request, &value)
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "availability-provider",
            "version": PROVIDER_VERSION,
            "targets": self.targets.iter().map(|target| {
                json!({
                    "id": target.id,
                    "ensure_url": target.ensure_url,
                    "configured": true,
                })
            }).collect::<Vec<_>>()
        }))
    }
}

fn parse_config(config: Value) -> Result<AvailabilityConfig, String> {
    let payload = config
        .get("extra")
        .filter(|extra| !extra.is_null())
        .unwrap_or(&config)
        .clone();
    let parsed: AvailabilityConfig =
        serde_json::from_value(payload).map_err(|err| err.to_string())?;
    validate_config(parsed)
}

fn validate_config(config: AvailabilityConfig) -> Result<AvailabilityConfig, String> {
    if config.targets.is_empty() {
        return Err("availability-provider requires at least one target".to_string());
    }
    if config.timeout_secs == 0 || config.timeout_secs > 300 {
        return Err("availability-provider timeout_secs must be between 1 and 300".to_string());
    }
    for target in &config.targets {
        if target.id.trim().is_empty() {
            return Err("availability target id must not be empty".to_string());
        }
        if !is_allowed_target_url(&target.ensure_url) {
            return Err(format!(
                "availability target '{}' must use https or local loopback http",
                target.id
            ));
        }
        if let Some(value) = &target.authorization {
            validate_header_value(&target.id, "authorization", value)?;
        }
        for name in target.headers.keys() {
            if !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            {
                return Err(format!(
                    "availability target '{}' has invalid header name '{}'",
                    target.id, name
                ));
            }
        }
        for (name, value) in &target.headers {
            validate_header_value(&target.id, name, value)?;
        }
    }
    Ok(config)
}

fn is_allowed_target_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")),
        _ => false,
    }
}

fn validate_header_value(target_id: &str, name: &str, value: &str) -> Result<(), String> {
    if value.bytes().any(|b| matches!(b, b'\r' | b'\n')) {
        return Err(format!(
            "availability target '{target_id}' has invalid header value for '{name}'"
        ));
    }
    Ok(())
}

fn normalize_upstream_availability(
    target: &AvailabilityTarget,
    request: &EnsureRequest,
    response: &Value,
) -> Result<Value, String> {
    if response.get("status").and_then(Value::as_str) == Some("error") {
        return Err(response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("availability target returned error")
            .to_string());
    }

    let data = response.get("data").unwrap_or(response);
    let availability = data.get("availability").unwrap_or(data);
    let status = availability
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| "availability target response missing status".to_string())?;
    let replicas = availability
        .get("replicas")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| local_replicas(request));
    let provider = availability
        .get("provider")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&target.id);
    let policy = availability
        .get("policy")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&request.policy);

    match status {
        "network_available" if replicas > 0 => Ok(json!({
            "status": status,
            "provider": provider,
            "policy": policy,
            "replicas": replicas,
        })),
        "network_available" => Err("network_available requires replicas > 0".to_string()),
        "repair_needed" => Ok(json!({
            "status": status,
            "provider": provider,
            "policy": policy,
            "replicas": replicas,
            "reason": availability.get("reason").and_then(Value::as_str).unwrap_or("availability target requested repair"),
        })),
        other => Err(format!("unsupported availability status: {other}")),
    }
}

fn repair_needed(provider: &str, request: &EnsureRequest, reason: impl Into<String>) -> Value {
    json!({
        "status": "repair_needed",
        "provider": provider,
        "policy": request.policy,
        "replicas": local_replicas(request),
        "reason": reason.into(),
    })
}

fn local_replicas(request: &EnsureRequest) -> u64 {
    request
        .local
        .get("replicas")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn upstream_error_message(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            if body.trim().is_empty() {
                format!("HTTP {code}")
            } else {
                format!("HTTP {code}: {}", body.trim())
            }
        }
        ureq::Error::Transport(err) => err.to_string(),
    }
}

fn default_timeout_secs() -> u64 {
    30
}

fn main() {
    eprintln!(
        "availability-provider: starting v{} (configured targets only)",
        PROVIDER_VERSION
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = AvailabilityProvider::new();

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

    eprintln!("availability-provider: exiting");
}

fn write_response(stdout: &mut io::Stdout, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> AvailabilityTarget {
        AvailabilityTarget {
            id: "elacity-supernode".to_string(),
            ensure_url: "https://example.invalid/availability/ensure".to_string(),
            authorization: None,
            headers: BTreeMap::new(),
        }
    }

    fn request() -> EnsureRequest {
        EnsureRequest {
            cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string(),
            uri: "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
                .to_string(),
            policy: "network_default".to_string(),
            local: json!({"status": "local_pinned", "replicas": 1}),
            object_did: None,
            publisher_did: None,
        }
    }

    #[test]
    fn config_requires_targets_and_secure_urls() {
        let err = parse_config(json!({"extra": {"targets": []}})).unwrap_err();
        assert!(err.contains("requires at least one target"));

        let err = parse_config(json!({
            "extra": {
                "targets": [{"id": "remote", "ensure_url": "http://example.com/ensure"}]
            }
        }))
        .unwrap_err();
        assert!(err.contains("https or local loopback"));

        let err = parse_config(json!({
            "extra": {
                "targets": [{"id": "remote", "ensure_url": "http://localhost.example/ensure"}]
            }
        }))
        .unwrap_err();
        assert!(err.contains("https or local loopback"));

        let err = parse_config(json!({
            "extra": {
                "targets": [{
                    "id": "local",
                    "ensure_url": "http://localhost:9080/ensure",
                    "headers": {"X-Test": "ok\nbad"}
                }]
            }
        }))
        .unwrap_err();
        assert!(err.contains("invalid header value"));
    }

    #[test]
    fn config_accepts_extra_shape() {
        let config = parse_config(json!({
            "base_path": "",
            "extra": {
                "timeout_secs": 5,
                "targets": [{
                    "id": "local-supernode",
                    "ensure_url": "http://127.0.0.1:9080/availability/ensure",
                    "authorization": "Bearer test"
                }]
            }
        }))
        .unwrap();

        assert_eq!(config.timeout_secs, 5);
        assert_eq!(config.targets[0].id, "local-supernode");
        assert_eq!(
            config.targets[0].authorization.as_deref(),
            Some("Bearer test")
        );
    }

    #[test]
    fn upstream_network_available_normalizes() {
        let availability = normalize_upstream_availability(
            &target(),
            &request(),
            &json!({
                "status": "ok",
                "data": {
                    "availability": {
                        "status": "network_available",
                        "provider": "elacity",
                        "replicas": 3
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(availability["status"], "network_available");
        assert_eq!(availability["provider"], "elacity");
        assert_eq!(availability["policy"], "network_default");
        assert_eq!(availability["replicas"], 3);
    }

    #[test]
    fn upstream_repair_needed_preserves_reason() {
        let availability = normalize_upstream_availability(
            &target(),
            &request(),
            &json!({
                "availability": {
                    "status": "repair_needed",
                    "reason": "not pinned by target yet"
                }
            }),
        )
        .unwrap();

        assert_eq!(availability["status"], "repair_needed");
        assert_eq!(availability["provider"], "elacity-supernode");
        assert_eq!(availability["replicas"], 1);
        assert_eq!(availability["reason"], "not pinned by target yet");
    }

    #[test]
    fn upstream_network_available_requires_replicas() {
        let err = normalize_upstream_availability(
            &target(),
            &request(),
            &json!({"availability": {"status": "network_available", "replicas": 0}}),
        )
        .unwrap_err();

        assert!(err.contains("replicas > 0"));
    }

    #[test]
    fn ensure_wire_request_rejects_hidden_provider_authority_fields() {
        let mut payload = serde_json::to_value(request()).unwrap();
        payload.as_object_mut().unwrap().insert(
            "elacity_sdk_token".to_string(),
            json!("must-not-be-accepted"),
        );

        let err = serde_json::from_value::<Request>(json!({
            "op": "ensure",
            "cid": payload["cid"].clone(),
            "uri": payload["uri"].clone(),
            "policy": payload["policy"].clone(),
            "local": payload["local"].clone(),
            "elacity_sdk_token": payload["elacity_sdk_token"].clone()
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }
}
