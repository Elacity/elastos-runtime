//! ElastOS Operator Drive Adapter Capsule
//!
//! This is a real WebSpace resolver adapter package, not a Library UI helper.
//! It only accepts metadata/read/write calls when Runtime injects an explicit
//! provider invocation envelope, and it stores bytes in a provider-owned local
//! fixture namespace for deterministic development and release tests.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const ADAPTER_SCHEMA: &str = "elastos.webspace.operator-drive-adapter/v1";
const TARGET_PREFIX: &str = "operator://drive";
const PROVIDER_NAME: &str = "operator-drive-adapter";
const RESOLVER_NAME: &str = "operator-drive";
const SEED_BRIEF_TARGET: &str = "operator://drive/Projects/Brief.md";
const SEED_BRIEF_BYTES: &[u8] = b"# Operator Brief\n\nAdapter-backed bytes.\n";
const ENDPOINT_REQUEST_SCHEMA: &str = "elastos.webspace.operator-endpoint.request/v1";
const DEFAULT_ENDPOINT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: ProviderConfig,
    },
    Status,
    MetadataIndex {
        #[serde(default)]
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        #[serde(default, rename = "_runtime_invocation")]
        runtime_invocation: Option<Value>,
    },
    ReadBytes {
        #[serde(default)]
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        #[serde(default, rename = "_runtime_invocation")]
        runtime_invocation: Option<Value>,
    },
    WriteBytes {
        #[serde(default)]
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        data: String,
        #[serde(default)]
        if_head: Option<String>,
        #[serde(default, rename = "_runtime_invocation")]
        runtime_invocation: Option<Value>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProviderConfig {
    base_path: String,
    extra: Value,
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

#[derive(Debug)]
struct OperatorDriveAdapter {
    root: PathBuf,
    endpoint: Option<OperatorEndpoint>,
}

impl Default for OperatorDriveAdapter {
    fn default() -> Self {
        Self {
            root: std::env::temp_dir().join("elastos-operator-drive-adapter"),
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone)]
struct OperatorEndpoint {
    host: String,
    port: u16,
    path: String,
    authorization: Option<String>,
    timeout: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorEndpointConfig {
    url: String,
    #[serde(default)]
    authorization: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug)]
struct EndpointFailure {
    code: String,
    message: String,
}

impl EndpointFailure {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl OperatorDriveAdapter {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::MetadataIndex {
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                runtime_invocation,
            } => self.metadata_index(
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                runtime_invocation,
            ),
            Request::ReadBytes {
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                runtime_invocation,
            } => self.read_bytes(
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                runtime_invocation,
            ),
            Request::WriteBytes {
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                data,
                if_head,
                runtime_invocation,
            } => self.write_bytes(
                schema,
                mount,
                resolver,
                handle_uri,
                target_uri,
                data,
                if_head,
                runtime_invocation,
            ),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: ProviderConfig) -> Response {
        let root = config
            .extra
            .get("operator_drive_root")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| {
                if config.base_path.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(config.base_path).join("operator-drive-adapter"))
                }
            })
            .unwrap_or_else(|| self.root.clone());
        self.root = root;
        self.endpoint = match operator_endpoint_from_extra(&config.extra) {
            Ok(endpoint) => endpoint,
            Err(err) => return Response::error("invalid_endpoint_config", err),
        };
        if let Err(err) = fs::create_dir_all(self.object_root()) {
            return Response::error(
                "init_failed",
                format!("cannot initialize adapter store: {err}"),
            );
        }
        Response::ok(json!({
            "schema": ADAPTER_SCHEMA,
            "provider": PROVIDER_NAME,
            "resolver": RESOLVER_NAME,
            "version": PROVIDER_VERSION,
            "configured": true,
            "endpoint": endpoint_summary(self.endpoint.as_ref()),
            "capabilities": ["metadata_index", "read_bytes", "write_bytes"],
            "authority_boundary": "Runtime provider invocation only; no app-visible resolver credentials or host paths"
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "schema": ADAPTER_SCHEMA,
            "provider": PROVIDER_NAME,
            "resolver": RESOLVER_NAME,
            "version": PROVIDER_VERSION,
            "configured": true,
            "state": "connected",
            "endpoint": endpoint_summary(self.endpoint.as_ref()),
            "capabilities": ["metadata_index", "read_bytes", "write_bytes"],
            "target_prefix": TARGET_PREFIX,
            "blocked_authority": [
                "resolver_credentials",
                "host_paths",
                "raw_backend_sdk",
                "carrier_tickets",
                "kubo_ipfs_handles"
            ],
            "contract": {
                "schema": "elastos.webspace.adapter/v1",
                "operations": ["metadata_index", "read_bytes", "write_bytes"],
                "requires_runtime_invocation": true,
                "credential_policy": "provider-owned; never serialized to apps"
            }
        }))
    }

    fn metadata_index(
        &self,
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        runtime_invocation: Option<Value>,
    ) -> Response {
        if let Err(err) = validate_adapter_request(
            schema.as_deref(),
            &mount,
            &resolver,
            &handle_uri,
            &target_uri,
            runtime_invocation.as_ref(),
            "metadata_index",
        ) {
            return Response::error("invalid_request", err);
        }
        let parts = match target_parts(&target_uri) {
            Ok(parts) => parts,
            Err(err) => return Response::error("invalid_target", err),
        };
        if let Some(endpoint) = &self.endpoint {
            return self.metadata_index_via_endpoint(endpoint, mount, handle_uri, target_uri);
        }
        let mut entries = Vec::new();
        if parts.is_empty() {
            entries.push(index_entry(
                "Projects",
                "directory",
                "operator://drive/Projects",
                true,
            ));
            entries.push(index_entry(
                "Projects/Brief.md",
                "file",
                SEED_BRIEF_TARGET,
                true,
            ));
            entries.push(index_entry(
                "Writable",
                "directory",
                "operator://drive/Writable",
                false,
            ));
        } else if parts == ["Projects"] {
            entries.push(index_entry("Brief.md", "file", SEED_BRIEF_TARGET, true));
        }
        if let Err(err) = self.collect_stored_entries(&parts, &mut entries) {
            return Response::error("index_failed", err);
        }
        entries.sort_by(|left, right| {
            left.get("path")
                .and_then(Value::as_str)
                .cmp(&right.get("path").and_then(Value::as_str))
        });
        entries.dedup_by(|left, right| left.get("path") == right.get("path"));
        Response::ok(json!({
            "schema": "elastos.webspace.adapter.metadata-index/v1",
            "resolver": RESOLVER_NAME,
            "mount": mount,
            "handle_uri": handle_uri,
            "target_uri": target_uri,
            "entries": entries,
            "receipt": {
                "schema": "elastos.webspace.adapter.metadata-index-receipt/v1",
                "resolver": RESOLVER_NAME,
                "provider": PROVIDER_NAME,
                "entry_count": entries.len(),
                "credential_exposed": false
            }
        }))
    }

    fn read_bytes(
        &self,
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        runtime_invocation: Option<Value>,
    ) -> Response {
        if let Err(err) = validate_adapter_request(
            schema.as_deref(),
            &mount,
            &resolver,
            &handle_uri,
            &target_uri,
            runtime_invocation.as_ref(),
            "read_bytes",
        ) {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = target_parts(&target_uri) {
            return Response::error("invalid_target", err);
        }
        if let Some(endpoint) = &self.endpoint {
            return self.read_bytes_via_endpoint(endpoint, mount, handle_uri, target_uri);
        }
        match self.read_target_bytes(&target_uri) {
            Ok(bytes) => Response::ok(json!({
                "schema": "elastos.webspace.adapter.read-bytes/v1",
                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "mime": mime_for_target(&target_uri),
                "receipt": {
                    "schema": "elastos.webspace.adapter.read-bytes-receipt/v1",
                    "resolver": RESOLVER_NAME,
                    "provider": PROVIDER_NAME,
                    "target_uri": target_uri,
                    "handle_uri": handle_uri,
                    "bytes": bytes.len(),
                    "credential_exposed": false
                }
            })),
            Err(err) => Response::error("read_failed", err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_bytes(
        &self,
        schema: Option<String>,
        mount: String,
        resolver: String,
        handle_uri: String,
        target_uri: String,
        data: String,
        if_head: Option<String>,
        runtime_invocation: Option<Value>,
    ) -> Response {
        if let Err(err) = validate_adapter_request(
            schema.as_deref(),
            &mount,
            &resolver,
            &handle_uri,
            &target_uri,
            runtime_invocation.as_ref(),
            "write_bytes",
        ) {
            return Response::error("invalid_request", err);
        }
        if !target_uri.starts_with("operator://drive/Writable/") {
            return Response::error(
                "readonly",
                "operator adapter writes are limited to operator://drive/Writable/*",
            );
        }
        if target_uri.contains("/Conflict/") || if_head.as_deref() == Some("head:conflict") {
            return Response::error(
                "conflict",
                "operator adapter rejected stale mutable fork write",
            );
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Response::error("invalid_data", format!("data must be base64: {err}"))
            }
        };
        if let Some(endpoint) = &self.endpoint {
            return self.write_bytes_via_endpoint(
                endpoint,
                mount,
                handle_uri,
                target_uri,
                data,
                if_head,
                bytes.len(),
            );
        }
        let path = match self.target_path(&target_uri) {
            Ok(path) => path,
            Err(err) => return Response::error("invalid_target", err),
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return Response::error(
                    "write_failed",
                    format!("cannot create parent directory: {err}"),
                );
            }
        }
        if let Err(err) = fs::write(&path, &bytes) {
            return Response::error(
                "write_failed",
                format!("cannot write adapter object: {err}"),
            );
        }
        Response::ok(json!({
            "schema": "elastos.webspace.adapter.write-bytes/v1",
            "receipt": {
                "schema": "elastos.webspace.adapter.write-bytes-receipt/v1",
                "resolver": RESOLVER_NAME,
                "provider": PROVIDER_NAME,
                "target_uri": target_uri,
                "handle_uri": handle_uri,
                "bytes_accepted": bytes.len(),
                "credential_exposed": false
            }
        }))
    }

    fn metadata_index_via_endpoint(
        &self,
        endpoint: &OperatorEndpoint,
        mount: String,
        handle_uri: String,
        target_uri: String,
    ) -> Response {
        let remote = match post_operator_endpoint(
            endpoint,
            "metadata_index",
            json!({
                "mount": mount,
                "resolver": RESOLVER_NAME,
                "handle_uri": handle_uri,
                "target_uri": target_uri
            }),
        ) {
            Ok(data) => data,
            Err(err) => return Response::error(&err.code, err.message),
        };
        let entries = match sanitize_endpoint_entries(&remote) {
            Ok(entries) => entries,
            Err(err) => return Response::error("invalid_endpoint_response", err),
        };
        Response::ok(json!({
            "schema": "elastos.webspace.adapter.metadata-index/v1",
            "resolver": RESOLVER_NAME,
            "mount": mount,
            "handle_uri": handle_uri,
            "target_uri": target_uri,
            "entries": entries,
            "receipt": {
                "schema": "elastos.webspace.adapter.metadata-index-receipt/v1",
                "resolver": RESOLVER_NAME,
                "provider": PROVIDER_NAME,
                "entry_count": entries.len(),
                "federation_backend": "operator_private_http",
                "credential_exposed": false,
                "endpoint_authority_exposed": false
            }
        }))
    }

    fn read_bytes_via_endpoint(
        &self,
        endpoint: &OperatorEndpoint,
        mount: String,
        handle_uri: String,
        target_uri: String,
    ) -> Response {
        let remote = match post_operator_endpoint(
            endpoint,
            "read_bytes",
            json!({
                "mount": mount,
                "resolver": RESOLVER_NAME,
                "handle_uri": handle_uri,
                "target_uri": target_uri
            }),
        ) {
            Ok(data) => data,
            Err(err) => return Response::error(&err.code, err.message),
        };
        let data = match remote.get("data").and_then(Value::as_str) {
            Some(data) => data.to_string(),
            None => {
                return Response::error(
                    "invalid_endpoint_response",
                    "operator endpoint read response must include base64 data",
                )
            }
        };
        let bytes = match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Response::error(
                    "invalid_endpoint_response",
                    format!("operator endpoint returned invalid base64 data: {err}"),
                )
            }
        };
        let mime = remote
            .get("mime")
            .and_then(Value::as_str)
            .unwrap_or_else(|| mime_for_target(&target_uri));
        Response::ok(json!({
            "schema": "elastos.webspace.adapter.read-bytes/v1",
            "data": data,
            "mime": mime,
            "receipt": {
                "schema": "elastos.webspace.adapter.read-bytes-receipt/v1",
                "resolver": RESOLVER_NAME,
                "provider": PROVIDER_NAME,
                "target_uri": target_uri,
                "handle_uri": handle_uri,
                "bytes": bytes.len(),
                "federation_backend": "operator_private_http",
                "credential_exposed": false,
                "endpoint_authority_exposed": false
            }
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn write_bytes_via_endpoint(
        &self,
        endpoint: &OperatorEndpoint,
        mount: String,
        handle_uri: String,
        target_uri: String,
        data: String,
        if_head: Option<String>,
        byte_count: usize,
    ) -> Response {
        let remote = match post_operator_endpoint(
            endpoint,
            "write_bytes",
            json!({
                "mount": mount,
                "resolver": RESOLVER_NAME,
                "handle_uri": handle_uri,
                "target_uri": target_uri,
                "data": data,
                "if_head": if_head
            }),
        ) {
            Ok(data) => data,
            Err(err) => return Response::error(&err.code, err.message),
        };
        Response::ok(json!({
            "schema": "elastos.webspace.adapter.write-bytes/v1",
            "receipt": {
                "schema": "elastos.webspace.adapter.write-bytes-receipt/v1",
                "resolver": RESOLVER_NAME,
                "provider": PROVIDER_NAME,
                "target_uri": target_uri,
                "handle_uri": handle_uri,
                "bytes_accepted": byte_count,
                "remote_head": remote.get("head").cloned().unwrap_or(Value::Null),
                "federation_backend": "operator_private_http",
                "credential_exposed": false,
                "endpoint_authority_exposed": false
            }
        }))
    }

    fn object_root(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn target_path(&self, target_uri: &str) -> Result<PathBuf, String> {
        let parts = target_parts(target_uri)?;
        if parts.is_empty() {
            return Err("target URI must reference a file".to_string());
        }
        Ok(parts
            .into_iter()
            .fold(self.object_root(), |path, part| path.join(part)))
    }

    fn read_target_bytes(&self, target_uri: &str) -> Result<Vec<u8>, String> {
        if target_uri == SEED_BRIEF_TARGET {
            return Ok(SEED_BRIEF_BYTES.to_vec());
        }
        let path = self.target_path(target_uri)?;
        fs::read(path).map_err(|err| format!("operator adapter target is unavailable: {err}"))
    }

    fn collect_stored_entries(
        &self,
        prefix: &[String],
        entries: &mut Vec<Value>,
    ) -> Result<(), String> {
        let root = prefix
            .iter()
            .fold(self.object_root(), |path, part| path.join(part));
        if !root.exists() {
            return Ok(());
        }
        collect_stored_entries_at(&self.object_root(), &root, prefix.len(), entries)
    }
}

fn validate_adapter_request(
    schema: Option<&str>,
    mount: &str,
    resolver: &str,
    handle_uri: &str,
    target_uri: &str,
    runtime_invocation: Option<&Value>,
    op: &str,
) -> Result<(), String> {
    let op_schema = op.replace('_', "-");
    let expected_schema = format!("elastos.webspace.adapter.{op_schema}-request/v1");
    if schema.unwrap_or(expected_schema.as_str()) != expected_schema {
        return Err(format!("adapter request schema mismatch for {op}"));
    }
    require_non_empty(mount, "mount")?;
    if resolver != RESOLVER_NAME {
        return Err(format!("resolver must be {RESOLVER_NAME}"));
    }
    if !handle_uri.starts_with("localhost://WebSpaces/") {
        return Err("handle_uri must be a localhost WebSpaces handle".to_string());
    }
    target_parts(target_uri)?;
    require_runtime_invocation(runtime_invocation, op)
}

fn require_runtime_invocation(invocation: Option<&Value>, op: &str) -> Result<(), String> {
    let Some(invocation) = invocation.and_then(Value::as_object) else {
        return Err("operator adapter requires Runtime provider invocation".to_string());
    };
    if invocation.get("schema").and_then(Value::as_str) != Some("elastos.provider.invocation/v1") {
        return Err("runtime invocation schema is unsupported".to_string());
    }
    if invocation.get("source").and_then(Value::as_str) != Some("webspace-provider") {
        return Err("runtime invocation source must be webspace-provider".to_string());
    }
    if invocation.get("target").and_then(Value::as_str) != Some(PROVIDER_NAME) {
        return Err("runtime invocation target must be operator-drive-adapter".to_string());
    }
    if invocation.get("op").and_then(Value::as_str) != Some(op) {
        return Err("runtime invocation op mismatch".to_string());
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(())
    }
}

fn target_parts(target_uri: &str) -> Result<Vec<String>, String> {
    let rest = target_uri
        .strip_prefix(TARGET_PREFIX)
        .ok_or_else(|| format!("target_uri must start with {TARGET_PREFIX}"))?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.split('/')
        .map(|segment| {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains('\\')
                || segment.contains('\0')
            {
                Err("target_uri contains an unsafe path segment".to_string())
            } else {
                Ok(segment.to_string())
            }
        })
        .collect()
}

fn index_entry(path: &str, kind: &str, target_uri: &str, readonly: bool) -> Value {
    json!({
        "path": path,
        "kind": kind,
        "target_uri": target_uri,
        "resolver_state": "indexed",
        "readonly": readonly,
        "description": "Operator drive adapter entry."
    })
}

fn operator_endpoint_from_extra(extra: &Value) -> Result<Option<OperatorEndpoint>, String> {
    let Some(raw) = extra.get("operator_endpoint") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let config: OperatorEndpointConfig = serde_json::from_value(raw.clone())
        .map_err(|err| format!("invalid operator_endpoint config: {err}"))?;
    parse_operator_endpoint(config)
}

fn parse_operator_endpoint(
    config: OperatorEndpointConfig,
) -> Result<Option<OperatorEndpoint>, String> {
    let url = config.url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    if url.contains('?') || url.contains('#') {
        return Err("operator endpoint URL must not contain query or fragment".to_string());
    }
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        "operator endpoint backend only supports operator-private http:// loopback URLs".to_string()
    })?;
    let (authority, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return Err("operator endpoint URL must not contain userinfo credentials".to_string());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| "operator endpoint URL has an invalid port".to_string())?;
            (host.to_string(), port)
        }
        None => (authority.to_string(), 80),
    };
    if host != "127.0.0.1" && host != "localhost" {
        return Err(
            "operator endpoint backend must use a loopback host owned by the operator service"
                .to_string(),
        );
    }
    let path = if raw_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{raw_path}")
    };
    let authorization = config
        .authorization
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let timeout_ms = config
        .timeout_ms
        .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_MS)
        .clamp(100, 30_000);
    Ok(Some(OperatorEndpoint {
        host,
        port,
        path,
        authorization,
        timeout: Duration::from_millis(timeout_ms),
    }))
}

fn endpoint_summary(endpoint: Option<&OperatorEndpoint>) -> Value {
    match endpoint {
        Some(endpoint) => json!({
            "schema": "elastos.webspace.operator-endpoint.summary/v1",
            "configured": true,
            "mode": "operator_private_http",
            "scheme": "http",
            "loopback_only": true,
            "authorization_configured": endpoint.authorization.is_some(),
            "credential_exposed": false,
            "endpoint_authority_exposed": false,
            "note": "Endpoint URL and credentials are provider-owned and redacted from app-visible status."
        }),
        None => json!({
            "schema": "elastos.webspace.operator-endpoint.summary/v1",
            "configured": false,
            "mode": "deterministic_local_store",
            "credential_exposed": false,
            "endpoint_authority_exposed": false
        }),
    }
}

fn post_operator_endpoint(
    endpoint: &OperatorEndpoint,
    op: &str,
    request: Value,
) -> Result<Value, EndpointFailure> {
    let envelope = json!({
        "schema": ENDPOINT_REQUEST_SCHEMA,
        "op": op,
        "provider": PROVIDER_NAME,
        "resolver": RESOLVER_NAME,
        "target_prefix": TARGET_PREFIX,
        "request": request
    });
    let body = serde_json::to_string(&envelope).map_err(|err| {
        EndpointFailure::new(
            "operator_endpoint_request_failed",
            format!("cannot encode operator endpoint request: {err}"),
        )
    })?;
    let address_host = if endpoint.host == "localhost" {
        "127.0.0.1"
    } else {
        endpoint.host.as_str()
    };
    let mut addresses = format!("{address_host}:{}", endpoint.port)
        .to_socket_addrs()
        .map_err(|err| {
            EndpointFailure::new(
                "operator_endpoint_unavailable",
                format!("cannot resolve operator endpoint: {err}"),
            )
        })?;
    let address = addresses
        .find(|address| address.ip().is_loopback())
        .ok_or_else(|| {
            EndpointFailure::new(
                "operator_endpoint_unavailable",
                "operator endpoint did not resolve to loopback",
            )
        })?;
    let mut stream = TcpStream::connect_timeout(&address, endpoint.timeout).map_err(|err| {
        EndpointFailure::new(
            "operator_endpoint_unavailable",
            format!("cannot connect to operator endpoint: {err}"),
        )
    })?;
    let _ = stream.set_read_timeout(Some(endpoint.timeout));
    let _ = stream.set_write_timeout(Some(endpoint.timeout));
    let mut wire_request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len()
    );
    if let Some(authorization) = &endpoint.authorization {
        wire_request.push_str("Authorization: ");
        wire_request.push_str(authorization);
        wire_request.push_str("\r\n");
    }
    wire_request.push_str("\r\n");
    wire_request.push_str(&body);
    stream.write_all(wire_request.as_bytes()).map_err(|err| {
        EndpointFailure::new(
            "operator_endpoint_request_failed",
            format!("cannot write operator endpoint request: {err}"),
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|err| {
        EndpointFailure::new(
            "operator_endpoint_request_failed",
            format!("cannot read operator endpoint response: {err}"),
        )
    })?;
    parse_operator_http_response(&response)
}

fn parse_operator_http_response(response: &str) -> Result<Value, EndpointFailure> {
    let (headers, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        EndpointFailure::new(
            "operator_endpoint_response_invalid",
            "operator endpoint response is missing HTTP headers",
        )
    })?;
    let status_line = headers.lines().next().unwrap_or_default();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| {
            EndpointFailure::new(
                "operator_endpoint_response_invalid",
                "operator endpoint response has invalid HTTP status",
            )
        })?;
    if !(200..300).contains(&status_code) {
        return Err(EndpointFailure::new(
            "operator_endpoint_http_error",
            format!("operator endpoint returned HTTP {status_code}"),
        ));
    }
    let payload: Value = serde_json::from_str(body.trim()).map_err(|err| {
        EndpointFailure::new(
            "operator_endpoint_response_invalid",
            format!("operator endpoint response is not JSON: {err}"),
        )
    })?;
    match payload.get("status").and_then(Value::as_str) {
        Some("ok") => Ok(payload.get("data").cloned().unwrap_or(Value::Null)),
        Some("error") => Err(EndpointFailure::new(
            payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("operator_endpoint_error"),
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("operator endpoint returned an error"),
        )),
        _ => Err(EndpointFailure::new(
            "operator_endpoint_response_invalid",
            "operator endpoint response must use status ok or error",
        )),
    }
}

fn sanitize_endpoint_entries(remote: &Value) -> Result<Vec<Value>, String> {
    let entries = remote
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "operator endpoint metadata response must include entries".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "operator endpoint entry missing path".to_string())?;
            let kind = entry
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| "operator endpoint entry missing kind".to_string())?;
            if !matches!(kind, "file" | "directory") {
                return Err("operator endpoint entry kind must be file or directory".to_string());
            }
            let target_uri = entry
                .get("target_uri")
                .and_then(Value::as_str)
                .ok_or_else(|| "operator endpoint entry missing target_uri".to_string())?;
            target_parts(target_uri)?;
            let readonly = entry
                .get("readonly")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(json!({
                "path": path,
                "kind": kind,
                "target_uri": target_uri,
                "resolver_state": "endpoint_indexed",
                "readonly": readonly,
                "description": entry
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("Operator endpoint adapter entry.")
            }))
        })
        .collect()
}

fn collect_stored_entries_at(
    object_root: &Path,
    current: &Path,
    prefix_len: usize,
    entries: &mut Vec<Value>,
) -> Result<(), String> {
    let dir = fs::read_dir(current).map_err(|err| format!("cannot read adapter store: {err}"))?;
    for entry in dir {
        let entry = entry.map_err(|err| format!("cannot read adapter store entry: {err}"))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|err| format!("cannot read adapter store metadata: {err}"))?;
        let relative = path
            .strip_prefix(object_root)
            .map_err(|_| "adapter store path escaped object root".to_string())?
            .iter()
            .map(|part| part.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let display = relative
            .iter()
            .skip(prefix_len)
            .cloned()
            .collect::<Vec<_>>()
            .join("/");
        if !display.is_empty() {
            let target_uri = format!("{TARGET_PREFIX}/{}", relative.join("/"));
            entries.push(index_entry(
                &display,
                if metadata.is_dir() {
                    "directory"
                } else {
                    "file"
                },
                &target_uri,
                false,
            ));
        }
        if metadata.is_dir() {
            collect_stored_entries_at(object_root, &path, prefix_len, entries)?;
        }
    }
    Ok(())
}

fn mime_for_target(target_uri: &str) -> &'static str {
    let lower = target_uri.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        "text/plain"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

fn write_response(response: &Response) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn main() {
    let stdin = io::stdin();
    let mut adapter = OperatorDriveAdapter::default();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                let _ = write_response(&Response::error("io_error", err.to_string()));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => adapter.handle(request),
            Err(err) => Response::error("invalid_request", err.to_string()),
        };
        let shutdown = matches!(response, Response::Ok { .. })
            && serde_json::from_str::<Request>(&line)
                .map(|request| matches!(request, Request::Shutdown))
                .unwrap_or(false);
        if let Err(err) = write_response(&response) {
            eprintln!("operator-drive-adapter write error: {err}");
            break;
        }
        if shutdown {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    fn runtime_invocation(op: &str) -> Value {
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "webspace-provider",
            "target": PROVIDER_NAME,
            "op": op
        })
    }

    fn init_with_temp(provider: &mut OperatorDriveAdapter) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let response = provider.init(ProviderConfig {
            base_path: dir.path().display().to_string(),
            extra: Value::Null,
        });
        assert!(matches!(response, Response::Ok { .. }));
        dir
    }

    fn init_with_endpoint(
        provider: &mut OperatorDriveAdapter,
        endpoint_url: &str,
        authorization: &str,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let response = provider.init(ProviderConfig {
            base_path: dir.path().display().to_string(),
            extra: json!({
                "operator_endpoint": {
                    "url": endpoint_url,
                    "authorization": authorization,
                    "timeout_ms": 1_000
                }
            }),
        });
        assert!(matches!(response, Response::Ok { .. }));
        dir
    }

    fn spawn_operator_endpoint(
        response_data: Value,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            sender.send(request).unwrap();
            let body = json!({
                "status": "ok",
                "data": response_data
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/operator-drive"), receiver, handle)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 512];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(header_end) = buffer.windows(4).position(|item| item == b"\r\n\r\n") {
                let header_end = header_end + 4;
                let headers = String::from_utf8_lossy(&buffer[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);
                if buffer.len() >= header_end + content_length {
                    break;
                }
            }
        }
        String::from_utf8(buffer).unwrap()
    }

    fn request_body(request: &str) -> Value {
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    fn spawn_filesystem_operator_endpoint(
        root: PathBuf,
        request_count: usize,
    ) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                let envelope = request_body(&request);
                sender.send(envelope.clone()).unwrap();
                let data = filesystem_endpoint_response(&root, &envelope);
                let body = json!({
                    "status": "ok",
                    "data": data
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}/operator-drive"), receiver, handle)
    }

    fn filesystem_endpoint_response(root: &Path, envelope: &Value) -> Value {
        assert_eq!(envelope["schema"], ENDPOINT_REQUEST_SCHEMA);
        assert_eq!(envelope["provider"], PROVIDER_NAME);
        assert_eq!(envelope["resolver"], RESOLVER_NAME);
        assert_eq!(envelope["target_prefix"], TARGET_PREFIX);
        assert!(envelope.get("_runtime_invocation").is_none());
        let request = &envelope["request"];
        match envelope["op"].as_str().unwrap() {
            "metadata_index" => json!({
                "entries": filesystem_endpoint_entries(root)
            }),
            "read_bytes" => {
                let target_uri = request["target_uri"].as_str().unwrap();
                let bytes = fs::read(filesystem_endpoint_path(root, target_uri)).unwrap();
                json!({
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                    "mime": mime_for_target(target_uri)
                })
            }
            "write_bytes" => {
                let target_uri = request["target_uri"].as_str().unwrap();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(request["data"].as_str().unwrap())
                    .unwrap();
                let path = filesystem_endpoint_path(root, target_uri);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, bytes).unwrap();
                json!({
                    "head": format!("fs-head:{}", target_uri.replace('/', ":"))
                })
            }
            op => panic!("unsupported filesystem endpoint op: {op}"),
        }
    }

    fn filesystem_endpoint_entries(root: &Path) -> Vec<Value> {
        let mut entries = Vec::new();
        filesystem_endpoint_collect(root, root, &mut entries);
        entries.sort_by(|left, right| {
            left["path"]
                .as_str()
                .unwrap()
                .cmp(right["path"].as_str().unwrap())
        });
        entries
    }

    fn filesystem_endpoint_collect(root: &Path, current: &Path, entries: &mut Vec<Value>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .iter()
                .map(|part| part.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            if relative.is_empty() {
                continue;
            }
            let target_uri = format!("{TARGET_PREFIX}/{relative}");
            entries.push(json!({
                "path": relative,
                "kind": if metadata.is_dir() { "directory" } else { "file" },
                "target_uri": target_uri,
                "readonly": !target_uri.starts_with("operator://drive/Writable/"),
                "description": "Filesystem-backed operator endpoint entry."
            }));
            if metadata.is_dir() {
                filesystem_endpoint_collect(root, &path, entries);
            }
        }
    }

    fn filesystem_endpoint_path(root: &Path, target_uri: &str) -> PathBuf {
        let parts = target_parts(target_uri).unwrap();
        assert!(!parts.is_empty());
        parts
            .into_iter()
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }

    fn data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            Response::Ok { data: None } => Value::Null,
            Response::Error { code, message } => panic!("{code}: {message}"),
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            Response::Ok { .. } => panic!("expected error"),
        }
    }

    #[test]
    fn status_exposes_contract_without_credentials() {
        let provider = OperatorDriveAdapter::default();
        let data = data(provider.status());
        assert_eq!(data["schema"], ADAPTER_SCHEMA);
        assert_eq!(data["provider"], PROVIDER_NAME);
        assert_eq!(data["resolver"], RESOLVER_NAME);
        assert_eq!(data["contract"]["requires_runtime_invocation"], true);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "resolver_credentials"));
    }

    #[test]
    fn status_redacts_operator_endpoint_authority() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_endpoint(
            &mut provider,
            "http://127.0.0.1:39999/operator-secret-path",
            "Bearer endpoint-secret",
        );
        let data = data(provider.status());
        assert_eq!(data["endpoint"]["configured"], true);
        assert_eq!(data["endpoint"]["mode"], "operator_private_http");
        assert_eq!(data["endpoint"]["authorization_configured"], true);
        assert_eq!(data["endpoint"]["credential_exposed"], false);
        assert_eq!(data["endpoint"]["endpoint_authority_exposed"], false);
        let serialized = data.to_string();
        assert!(!serialized.contains("endpoint-secret"));
        assert!(!serialized.contains("127.0.0.1"));
        assert!(!serialized.contains("39999"));
        assert!(!serialized.contains("operator-secret-path"));
    }

    #[test]
    fn endpoint_config_rejects_non_loopback_backend() {
        let mut provider = OperatorDriveAdapter::default();
        let dir = tempfile::tempdir().unwrap();
        let response = provider.init(ProviderConfig {
            base_path: dir.path().display().to_string(),
            extra: json!({
                "operator_endpoint": {
                    "url": "http://example.com/operator-drive"
                }
            }),
        });
        assert_eq!(error_code(response), "invalid_endpoint_config");
    }

    #[test]
    fn metadata_index_requires_runtime_invocation() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_temp(&mut provider);
        assert_eq!(
            error_code(provider.metadata_index(
                None,
                "Operator".to_string(),
                RESOLVER_NAME.to_string(),
                "localhost://WebSpaces/Operator".to_string(),
                TARGET_PREFIX.to_string(),
                None,
            )),
            "invalid_request"
        );
    }

    #[test]
    fn metadata_index_lists_seeded_operator_entries() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_temp(&mut provider);
        let data = data(provider.metadata_index(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator".to_string(),
            TARGET_PREFIX.to_string(),
            Some(runtime_invocation("metadata_index")),
        ));
        assert_eq!(data["schema"], "elastos.webspace.adapter.metadata-index/v1");
        assert!(data["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "Projects/Brief.md"
                && entry["target_uri"] == SEED_BRIEF_TARGET));
        assert_eq!(data["receipt"]["credential_exposed"], false);
    }

    #[test]
    fn metadata_index_uses_operator_endpoint_backend_without_runtime_leakage() {
        let (endpoint_url, request_rx, handle) = spawn_operator_endpoint(json!({
            "entries": [
                {
                    "path": "Projects/Federated.md",
                    "kind": "file",
                    "target_uri": "operator://drive/Projects/Federated.md",
                    "readonly": true,
                    "description": "Federated operator endpoint entry."
                }
            ]
        }));
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_endpoint(&mut provider, &endpoint_url, "Bearer endpoint-secret");
        let data = data(provider.metadata_index(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator".to_string(),
            TARGET_PREFIX.to_string(),
            Some(runtime_invocation("metadata_index")),
        ));
        let request = request_rx.recv().unwrap();
        handle.join().unwrap();
        assert!(request.contains("Authorization: Bearer endpoint-secret"));
        assert!(request.contains(r#""schema":"elastos.webspace.operator-endpoint.request/v1""#));
        assert!(request.contains(r#""op":"metadata_index""#));
        assert!(!request.contains("_runtime_invocation"));
        assert!(!request.contains("elastos.provider.invocation/v1"));
        assert_eq!(
            data["receipt"]["federation_backend"],
            "operator_private_http"
        );
        assert_eq!(data["receipt"]["credential_exposed"], false);
        assert_eq!(data["receipt"]["endpoint_authority_exposed"], false);
        assert!(data["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "Projects/Federated.md"
                && entry["resolver_state"] == "endpoint_indexed"));
        assert!(!data.to_string().contains("endpoint-secret"));
    }

    #[test]
    fn read_bytes_returns_seeded_brief() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_temp(&mut provider);
        let data = data(provider.read_bytes(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator/Projects/Brief.md".to_string(),
            SEED_BRIEF_TARGET.to_string(),
            Some(runtime_invocation("read_bytes")),
        ));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, SEED_BRIEF_BYTES);
        assert_eq!(data["receipt"]["credential_exposed"], false);
    }

    #[test]
    fn read_bytes_uses_operator_endpoint_backend() {
        let (endpoint_url, request_rx, handle) = spawn_operator_endpoint(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(b"federated bytes"),
            "mime": "text/plain"
        }));
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_endpoint(&mut provider, &endpoint_url, "Bearer endpoint-secret");
        let data = data(provider.read_bytes(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator/Projects/Federated.md".to_string(),
            "operator://drive/Projects/Federated.md".to_string(),
            Some(runtime_invocation("read_bytes")),
        ));
        let request = request_rx.recv().unwrap();
        handle.join().unwrap();
        assert!(request.contains(r#""op":"read_bytes""#));
        assert!(!request.contains("_runtime_invocation"));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, b"federated bytes");
        assert_eq!(
            data["receipt"]["federation_backend"],
            "operator_private_http"
        );
        assert_eq!(data["receipt"]["endpoint_authority_exposed"], false);
    }

    #[test]
    fn write_bytes_persists_under_writable_namespace() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_temp(&mut provider);
        let target_uri = "operator://drive/Writable/Folder/note.txt";
        let write = data(provider.write_bytes(
            None,
            "OperatorMutable".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/OperatorMutable/Folder/note.txt".to_string(),
            target_uri.to_string(),
            base64::engine::general_purpose::STANDARD.encode(b"operator bytes"),
            None,
            Some(runtime_invocation("write_bytes")),
        ));
        assert_eq!(write["schema"], "elastos.webspace.adapter.write-bytes/v1");
        assert_eq!(write["receipt"]["bytes_accepted"], 14);

        let read = data(provider.read_bytes(
            None,
            "OperatorMutable".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/OperatorMutable/Folder/note.txt".to_string(),
            target_uri.to_string(),
            Some(runtime_invocation("read_bytes")),
        ));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(read["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, b"operator bytes");
    }

    #[test]
    fn write_bytes_uses_operator_endpoint_backend() {
        let (endpoint_url, request_rx, handle) = spawn_operator_endpoint(json!({
            "head": "remote-head-1"
        }));
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_endpoint(&mut provider, &endpoint_url, "Bearer endpoint-secret");
        let data = data(provider.write_bytes(
            None,
            "OperatorMutable".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/OperatorMutable/Folder/note.txt".to_string(),
            "operator://drive/Writable/Folder/note.txt".to_string(),
            base64::engine::general_purpose::STANDARD.encode(b"federated write"),
            None,
            Some(runtime_invocation("write_bytes")),
        ));
        let request = request_rx.recv().unwrap();
        handle.join().unwrap();
        assert!(request.contains(r#""op":"write_bytes""#));
        assert!(request.contains("ZmVkZXJhdGVkIHdyaXRl"));
        assert!(!request.contains("_runtime_invocation"));
        assert_eq!(data["receipt"]["remote_head"], "remote-head-1");
        assert_eq!(
            data["receipt"]["federation_backend"],
            "operator_private_http"
        );
        assert_eq!(data["receipt"]["bytes_accepted"], 15);
        assert_eq!(data["receipt"]["credential_exposed"], false);
    }

    #[test]
    fn operator_endpoint_backend_traverses_reads_and_writes_real_filesystem_state() {
        let backend = tempfile::tempdir().unwrap();
        let projects = backend.path().join("Projects");
        fs::create_dir_all(&projects).unwrap();
        fs::write(
            projects.join("Federated.md"),
            b"# Federated\n\nBackend bytes.\n",
        )
        .unwrap();
        let (endpoint_url, request_rx, handle) =
            spawn_filesystem_operator_endpoint(backend.path().to_path_buf(), 4);

        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_endpoint(&mut provider, &endpoint_url, "Bearer endpoint-secret");
        let index = data(provider.metadata_index(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator".to_string(),
            TARGET_PREFIX.to_string(),
            Some(runtime_invocation("metadata_index")),
        ));
        assert!(index["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "Projects/Federated.md"
                && entry["resolver_state"] == "endpoint_indexed"));
        assert_eq!(
            index["receipt"]["federation_backend"],
            "operator_private_http"
        );

        let read = data(provider.read_bytes(
            None,
            "Operator".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/Operator/Projects/Federated.md".to_string(),
            "operator://drive/Projects/Federated.md".to_string(),
            Some(runtime_invocation("read_bytes")),
        ));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(read["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, b"# Federated\n\nBackend bytes.\n");

        let write = data(provider.write_bytes(
            None,
            "OperatorMutable".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/OperatorMutable/Folder/note.txt".to_string(),
            "operator://drive/Writable/Folder/note.txt".to_string(),
            base64::engine::general_purpose::STANDARD.encode(b"backend write"),
            None,
            Some(runtime_invocation("write_bytes")),
        ));
        assert_eq!(write["receipt"]["bytes_accepted"], 13);
        assert_eq!(
            fs::read(backend.path().join("Writable/Folder/note.txt")).unwrap(),
            b"backend write"
        );

        let reread = data(provider.read_bytes(
            None,
            "OperatorMutable".to_string(),
            RESOLVER_NAME.to_string(),
            "localhost://WebSpaces/OperatorMutable/Folder/note.txt".to_string(),
            "operator://drive/Writable/Folder/note.txt".to_string(),
            Some(runtime_invocation("read_bytes")),
        ));
        let reread_bytes = base64::engine::general_purpose::STANDARD
            .decode(reread["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(reread_bytes, b"backend write");

        for _ in 0..4 {
            let envelope = request_rx.recv().unwrap();
            assert_eq!(envelope["schema"], ENDPOINT_REQUEST_SCHEMA);
            assert!(envelope.get("_runtime_invocation").is_none());
            assert!(!envelope.to_string().contains("endpoint-secret"));
            assert!(!envelope
                .to_string()
                .contains(backend.path().to_string_lossy().as_ref()));
        }
        handle.join().unwrap();
    }

    #[test]
    fn write_bytes_rejects_readonly_and_conflict_targets() {
        let mut provider = OperatorDriveAdapter::default();
        let _dir = init_with_temp(&mut provider);
        assert_eq!(
            error_code(provider.write_bytes(
                None,
                "Operator".to_string(),
                RESOLVER_NAME.to_string(),
                "localhost://WebSpaces/Operator/Projects/Brief.md".to_string(),
                SEED_BRIEF_TARGET.to_string(),
                base64::engine::general_purpose::STANDARD.encode(b"no"),
                None,
                Some(runtime_invocation("write_bytes")),
            )),
            "readonly"
        );
        assert_eq!(
            error_code(provider.write_bytes(
                None,
                "OperatorMutable".to_string(),
                RESOLVER_NAME.to_string(),
                "localhost://WebSpaces/OperatorMutable/Conflict/stale.txt".to_string(),
                "operator://drive/Writable/Conflict/stale.txt".to_string(),
                base64::engine::general_purpose::STANDARD.encode(b"stale"),
                None,
                Some(runtime_invocation("write_bytes")),
            )),
            "conflict"
        );
    }

    #[test]
    fn wire_request_rejects_hidden_credentials() {
        let payload = json!({
            "op": "read_bytes",
            "mount": "Operator",
            "resolver": RESOLVER_NAME,
            "handle_uri": "localhost://WebSpaces/Operator/Projects/Brief.md",
            "target_uri": SEED_BRIEF_TARGET,
            "_runtime_invocation": runtime_invocation("read_bytes"),
            "resolver_credentials": "must-not-be-accepted"
        });
        let err = serde_json::from_value::<Request>(payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"));
    }
}
