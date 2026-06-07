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
    requirements: Value,
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

#[derive(Debug, Clone, Copy)]
struct TargetAvailabilityRequirements {
    min_replicas: u64,
    max_replicas: Option<u64>,
    require_live_multi_peer_proof: bool,
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

        let requirements = TargetAvailabilityRequirements::from_value(&request.requirements);
        let mut network_available = Vec::new();
        let mut repair_needed_reports = Vec::new();
        let mut errors = Vec::new();
        for target in &self.targets {
            match self.ensure_target(target, &request) {
                Ok(availability) => {
                    if availability_status(&availability) == Some("network_available") {
                        network_available.push(availability);
                        if let Some(availability) = availability_for_requirements(
                            &request,
                            requirements,
                            &network_available,
                        ) {
                            return Response::ok(json!({ "availability": availability }));
                        }
                    } else {
                        repair_needed_reports.push(availability);
                    }
                }
                Err(err) => errors.push(format!("{}: {}", target.id, err)),
            }
        }

        if !network_available.is_empty() {
            return Response::ok(json!({
                "availability": repair_needed(
                    "availability-provider",
                    &request,
                    insufficient_target_reason(requirements, &network_available, &errors),
                )
            }));
        }

        if let Some(availability) = repair_needed_reports.into_iter().next() {
            return Response::ok(json!({ "availability": availability }));
        }

        Response::ok(json!({
            "availability": repair_needed(
                &self.targets[0].id,
                &request,
                errors
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "availability target failed".to_string()),
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

impl TargetAvailabilityRequirements {
    fn from_value(value: &Value) -> Self {
        Self {
            min_replicas: value
                .get("min_replicas")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1),
            max_replicas: value
                .get("max_replicas")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0),
            require_live_multi_peer_proof: value
                .get("require_live_multi_peer_proof")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
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
    let peer_selection = upstream_peer_selection(target, availability, replicas)?;
    let quota = upstream_quota(target, availability, request);
    let repair_worker = upstream_repair_worker(availability);
    let storage_market = upstream_storage_market(target, availability);
    let repair_graph = upstream_repair_graph(target, availability);
    let abuse_controls = upstream_abuse_controls(target, availability);

    match status {
        "network_available" if replicas > 0 => Ok(json!({
            "status": status,
            "provider": provider,
            "policy": policy,
            "replicas": replicas,
            "peer_selection": peer_selection,
            "quota": quota,
            "repair_worker": repair_worker,
            "storage_market": storage_market,
            "repair_graph": repair_graph,
            "abuse_controls": abuse_controls,
        })),
        "network_available" => Err("network_available requires replicas > 0".to_string()),
        "repair_needed" => Ok(json!({
            "status": status,
            "provider": provider,
            "policy": policy,
            "replicas": replicas,
            "reason": availability.get("reason").and_then(Value::as_str).unwrap_or("availability target requested repair"),
            "peer_selection": peer_selection,
            "quota": quota,
            "repair_worker": repair_worker,
            "storage_market": storage_market,
            "repair_graph": repair_graph,
            "abuse_controls": abuse_controls,
        })),
        other => Err(format!("unsupported availability status: {other}")),
    }
}

fn availability_for_requirements(
    request: &EnsureRequest,
    requirements: TargetAvailabilityRequirements,
    reports: &[Value],
) -> Option<Value> {
    let first = reports.first()?;
    if reports.len() == 1 && target_report_satisfies_requirements(first, requirements) {
        return Some(first.clone());
    }
    let aggregate = aggregate_target_availability(request, reports);
    if target_report_satisfies_requirements(&aggregate, requirements) {
        Some(aggregate)
    } else {
        None
    }
}

fn target_report_satisfies_requirements(
    availability: &Value,
    requirements: TargetAvailabilityRequirements,
) -> bool {
    if availability_status(availability) != Some("network_available") {
        return false;
    }
    let replicas = availability_replicas(availability);
    if replicas < requirements.min_replicas {
        return false;
    }
    if let Some(max_replicas) = requirements.max_replicas {
        if replicas > max_replicas {
            return false;
        }
    }
    if replicas > 1 && !availability_live_multi_peer_proof(availability) {
        return false;
    }
    if requirements.require_live_multi_peer_proof
        && !availability_live_multi_peer_proof(availability)
    {
        return false;
    }
    true
}

fn aggregate_target_availability(request: &EnsureRequest, reports: &[Value]) -> Value {
    let replicas = reports.iter().map(availability_replicas).sum::<u64>();
    let live_multi_peer_proof =
        reports.len() > 1 || reports.iter().any(availability_live_multi_peer_proof);
    let target_reports = reports
        .iter()
        .map(target_report_summary)
        .collect::<Vec<_>>();
    json!({
        "status": "network_available",
        "provider": "availability-provider",
        "policy": request.policy,
        "replicas": replicas,
        "peer_selection": {
            "mode": "configured_availability_target_fanout",
            "strategy": "target_fanout",
            "target_count": reports.len(),
            "live_multi_peer_proof": live_multi_peer_proof,
            "targets": target_reports,
        },
        "quota": {
            "policy": "configured_availability_target_fanout",
            "target_count": reports.len(),
            "requirements": request.requirements.clone(),
        },
        "repair_worker": {
            "scheduled": false,
            "status": "healthy",
            "worker": "availability-provider",
        },
        "storage_market": {
            "schema": "elastos.content.storage-market/v1",
            "mode": "configured_availability_target_fanout",
            "status": "target_reports_no_market_settlement",
            "settlement": "not_configured",
            "escrow": "not_configured",
            "quota_enforced": false,
            "target_count": reports.len(),
        },
        "repair_graph": {
            "schema": "elastos.content.repair-graph/v1",
            "policy": "configured_availability_target_fanout",
            "status": "target_reports_only",
            "supported": ["target_report"],
            "target_count": reports.len(),
        },
        "abuse_controls": {
            "schema": "elastos.content.abuse-controls/v1",
            "policy": "configured_availability_target_fanout",
            "enforced": reports.iter().any(|report| {
                report.get("abuse_controls")
                    .and_then(|value| value.get("enforced"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }),
            "throttled": reports.iter().any(|report| {
                report.get("abuse_controls")
                    .and_then(|value| value.get("throttled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }),
            "target_count": reports.len(),
        },
    })
}

fn target_report_summary(report: &Value) -> Value {
    json!({
        "provider": report.get("provider").cloned().unwrap_or(Value::Null),
        "policy": report.get("policy").cloned().unwrap_or(Value::Null),
        "status": report.get("status").cloned().unwrap_or(Value::Null),
        "replicas": availability_replicas(report),
        "peer_selection": report.get("peer_selection").cloned().unwrap_or(Value::Null),
        "quota": report.get("quota").cloned().unwrap_or(Value::Null),
        "repair_worker": report.get("repair_worker").cloned().unwrap_or(Value::Null),
        "storage_market": report.get("storage_market").cloned().unwrap_or(Value::Null),
        "repair_graph": report.get("repair_graph").cloned().unwrap_or(Value::Null),
        "abuse_controls": report.get("abuse_controls").cloned().unwrap_or(Value::Null),
    })
}

fn insufficient_target_reason(
    requirements: TargetAvailabilityRequirements,
    reports: &[Value],
    errors: &[String],
) -> String {
    let replicas = reports.iter().map(availability_replicas).sum::<u64>();
    let live_multi_peer_proof =
        reports.len() > 1 || reports.iter().any(availability_live_multi_peer_proof);
    let mut reason =
        format!("configured availability targets reported {replicas} replicas below requirements");
    if replicas < requirements.min_replicas {
        reason = format!(
            "configured availability targets reported {replicas} replicas below required {}",
            requirements.min_replicas
        );
    } else if let Some(max_replicas) = requirements.max_replicas {
        if replicas > max_replicas {
            reason = format!(
                "configured availability targets reported {replicas} replicas above quota {max_replicas}"
            );
        }
    }
    if requirements.require_live_multi_peer_proof && !live_multi_peer_proof {
        reason.push_str(" and no live multi-peer proof");
    }
    if let Some(last_error) = errors.last() {
        reason.push_str("; last target error: ");
        reason.push_str(last_error);
    }
    reason
}

fn availability_status(availability: &Value) -> Option<&str> {
    availability.get("status").and_then(Value::as_str)
}

fn availability_replicas(availability: &Value) -> u64 {
    availability
        .get("replicas")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn availability_live_multi_peer_proof(availability: &Value) -> bool {
    availability
        .get("peer_selection")
        .and_then(|value| value.get("live_multi_peer_proof"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn upstream_peer_selection(
    target: &AvailabilityTarget,
    availability: &Value,
    replicas: u64,
) -> Result<Value, String> {
    if let Some(peer_selection) = availability
        .get("peer_selection")
        .filter(|value| value.is_object())
        .cloned()
    {
        let live_multi_peer_proof = peer_selection
            .get("live_multi_peer_proof")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if replicas > 1 && !live_multi_peer_proof {
            return Err(
                "multi-replica availability target response requires live_multi_peer_proof=true"
                    .to_string(),
            );
        }
        return Ok(peer_selection);
    }
    if replicas > 1 {
        return Err(
            "multi-replica availability target response requires peer_selection metadata"
                .to_string(),
        );
    }
    Ok(json!({
        "mode": "configured_availability_target",
        "strategy": "target_report",
        "target_id": target.id,
        "live_multi_peer_proof": false,
    }))
}

fn upstream_quota(
    target: &AvailabilityTarget,
    availability: &Value,
    request: &EnsureRequest,
) -> Value {
    availability
        .get("quota")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "policy": "configured_availability_target",
                "target_id": target.id,
                "requirements": request.requirements.clone(),
            })
        })
}

fn upstream_repair_worker(availability: &Value) -> Value {
    availability
        .get("repair_worker")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "scheduled": false,
                "status": "healthy",
                "worker": "availability-provider",
            })
        })
}

fn upstream_storage_market(target: &AvailabilityTarget, availability: &Value) -> Value {
    availability
        .get("storage_market")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "schema": "elastos.content.storage-market/v1",
                "mode": "configured_availability_target",
                "status": "target_report_no_market_settlement",
                "target_id": target.id,
                "settlement": "not_configured",
                "escrow": "not_configured",
                "quota_enforced": false,
            })
        })
}

fn upstream_repair_graph(target: &AvailabilityTarget, availability: &Value) -> Value {
    availability
        .get("repair_graph")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "schema": "elastos.content.repair-graph/v1",
                "policy": "configured_availability_target",
                "status": "target_report_only",
                "target_id": target.id,
                "supported": ["target_report"],
            })
        })
}

fn upstream_abuse_controls(target: &AvailabilityTarget, availability: &Value) -> Value {
    availability
        .get("abuse_controls")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "schema": "elastos.content.abuse-controls/v1",
                "policy": "configured_availability_target",
                "target_id": target.id,
                "enforced": false,
                "throttled": false,
            })
        })
}

fn repair_needed(provider: &str, request: &EnsureRequest, reason: impl Into<String>) -> Value {
    json!({
        "status": "repair_needed",
        "provider": provider,
        "policy": request.policy,
        "replicas": local_replicas(request),
        "reason": reason.into(),
        "peer_selection": {
            "mode": "configured_availability_target",
            "strategy": "target_report",
            "live_multi_peer_proof": false,
        },
        "quota": {
            "policy": "configured_availability_target",
            "requirements": request.requirements.clone(),
        },
        "repair_worker": {
            "scheduled": true,
            "status": "queued",
            "worker": "availability-provider",
        },
        "storage_market": {
            "schema": "elastos.content.storage-market/v1",
            "mode": "configured_availability_target",
            "status": "target_report_no_market_settlement",
            "settlement": "not_configured",
            "escrow": "not_configured",
            "quota_enforced": false,
        },
        "repair_graph": {
            "schema": "elastos.content.repair-graph/v1",
            "policy": "configured_availability_target",
            "status": "target_report_only",
            "supported": ["target_report"],
        },
        "abuse_controls": {
            "schema": "elastos.content.abuse-controls/v1",
            "policy": "configured_availability_target",
            "enforced": false,
            "throttled": false,
        },
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
            requirements: json!({
                "min_replicas": 1,
                "max_replicas": null,
                "require_live_multi_peer_proof": false,
            }),
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
                        "replicas": 3,
                        "peer_selection": {
                            "mode": "supernode_cluster",
                            "live_multi_peer_proof": true
                        },
                        "quota": {
                            "policy": "operator_default",
                            "max_replicas": 3
                        },
                        "repair_worker": {
                            "scheduled": false,
                            "status": "healthy"
                        },
                        "storage_market": {
                            "schema": "elastos.content.storage-market/v1",
                            "mode": "supernode_cluster",
                            "status": "receipt_proven_no_market_settlement",
                            "settlement": "not_configured"
                        },
                        "repair_graph": {
                            "schema": "elastos.content.repair-graph/v1",
                            "policy": "supernode_cluster_graph",
                            "status": "available"
                        },
                        "abuse_controls": {
                            "schema": "elastos.content.abuse-controls/v1",
                            "policy": "supernode_cluster_guardrail",
                            "enforced": true,
                            "throttled": false
                        }
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(availability["status"], "network_available");
        assert_eq!(availability["provider"], "elacity");
        assert_eq!(availability["policy"], "network_default");
        assert_eq!(availability["replicas"], 3);
        assert_eq!(availability["peer_selection"]["mode"], "supernode_cluster");
        assert_eq!(
            availability["peer_selection"]["live_multi_peer_proof"],
            true
        );
        assert_eq!(availability["quota"]["max_replicas"], 3);
        assert_eq!(availability["repair_worker"]["status"], "healthy");
        assert_eq!(availability["storage_market"]["mode"], "supernode_cluster");
        assert_eq!(
            availability["storage_market"]["status"],
            "receipt_proven_no_market_settlement"
        );
        assert_eq!(
            availability["repair_graph"]["policy"],
            "supernode_cluster_graph"
        );
        assert_eq!(
            availability["abuse_controls"]["policy"],
            "supernode_cluster_guardrail"
        );
        assert_eq!(availability["abuse_controls"]["enforced"], true);
    }

    #[test]
    fn upstream_network_available_defaults_r4_policy_metadata() {
        let availability = normalize_upstream_availability(
            &target(),
            &request(),
            &json!({
                "availability": {
                    "status": "network_available",
                    "replicas": 1,
                    "peer_selection": {
                        "mode": "configured_availability_target",
                        "live_multi_peer_proof": false
                    },
                    "quota": {
                        "policy": "configured_availability_target"
                    },
                    "repair_worker": {
                        "status": "healthy"
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(
            availability["storage_market"]["schema"],
            "elastos.content.storage-market/v1"
        );
        assert_eq!(
            availability["storage_market"]["status"],
            "target_report_no_market_settlement"
        );
        assert_eq!(
            availability["repair_graph"]["schema"],
            "elastos.content.repair-graph/v1"
        );
        assert_eq!(availability["repair_graph"]["status"], "target_report_only");
        assert_eq!(
            availability["abuse_controls"]["schema"],
            "elastos.content.abuse-controls/v1"
        );
        assert_eq!(
            availability["abuse_controls"]["policy"],
            "configured_availability_target"
        );
    }

    #[test]
    fn configured_target_fanout_can_satisfy_live_multi_peer_requirement() {
        let mut request = request();
        request.requirements = json!({
            "min_replicas": 2,
            "max_replicas": 2,
            "require_live_multi_peer_proof": true
        });
        let mut backup = target();
        backup.id = "backup-supernode".to_string();
        let primary = normalize_upstream_availability(
            &target(),
            &request,
            &json!({
                "availability": {
                    "status": "network_available",
                    "replicas": 1
                }
            }),
        )
        .unwrap();
        let backup = normalize_upstream_availability(
            &backup,
            &request,
            &json!({
                "availability": {
                    "status": "network_available",
                    "replicas": 1
                }
            }),
        )
        .unwrap();

        let requirements = TargetAvailabilityRequirements::from_value(&request.requirements);
        let availability =
            availability_for_requirements(&request, requirements, &[primary, backup])
                .expect("two configured target reports should satisfy live proof");

        assert_eq!(availability["status"], "network_available");
        assert_eq!(availability["provider"], "availability-provider");
        assert_eq!(availability["replicas"], 2);
        assert_eq!(
            availability["peer_selection"]["mode"],
            "configured_availability_target_fanout"
        );
        assert_eq!(
            availability["peer_selection"]["live_multi_peer_proof"],
            true
        );
        assert_eq!(
            availability["peer_selection"]["targets"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            availability["storage_market"]["status"],
            "target_reports_no_market_settlement"
        );
    }

    #[test]
    fn configured_target_fanout_respects_max_replica_quota() {
        let mut request = request();
        request.requirements = json!({
            "min_replicas": 1,
            "max_replicas": 1,
            "require_live_multi_peer_proof": false
        });
        let availability = normalize_upstream_availability(
            &target(),
            &request,
            &json!({
                "availability": {
                    "status": "network_available",
                    "replicas": 2,
                    "peer_selection": {
                        "mode": "configured_availability_target",
                        "live_multi_peer_proof": true
                    }
                }
            }),
        )
        .unwrap();

        let requirements = TargetAvailabilityRequirements::from_value(&request.requirements);
        assert!(
            availability_for_requirements(&request, requirements, &[availability]).is_none(),
            "over-quota target reports must not become network_available"
        );
    }

    #[test]
    fn upstream_multi_replica_network_available_requires_peer_selection() {
        let err = normalize_upstream_availability(
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
        .unwrap_err();

        assert!(err.contains("requires peer_selection"));
    }

    #[test]
    fn upstream_multi_replica_network_available_requires_live_proof() {
        let err = normalize_upstream_availability(
            &target(),
            &request(),
            &json!({
                "availability": {
                    "status": "network_available",
                    "replicas": 2,
                    "peer_selection": {
                        "mode": "configured_availability_target",
                        "live_multi_peer_proof": false
                    }
                }
            }),
        )
        .unwrap_err();

        assert!(err.contains("live_multi_peer_proof=true"));
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
