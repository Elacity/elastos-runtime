//! Carrier-plane service bridge for WebSpace provider capsules.
//!
//! Carrier services are explicit host-plane exceptions for providers whose
//! contract fundamentally depends on host networking or host integration.
//! When a capsule declares `permissions.carrier: true`, Carrier may run it
//! directly on the host while preserving the same line-delimited JSON provider
//! contract exposed by VM-backed providers. Callers still see provider/resource
//! semantics, not host process or transport details.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use elastos_runtime::provider::{
    EntryType, Provider, ProviderError, ResourceAction, ResourceEntry, ResourceRequest,
    ResourceResponse,
};

struct ChildIo {
    reader: BufReader<std::process::ChildStdout>,
    writer: std::process::ChildStdin,
}

const CARRIER_SERVICE_READ_TIMEOUT: Duration = Duration::from_secs(15);

struct CarrierServiceBridge {
    init_config: serde_json::Value,
    io: Mutex<Option<ChildIo>>,
    /// Binary path and env vars for spawning.
    binary_path: String,
    env_vars: Vec<(String, String)>,
    child: Mutex<Option<Child>>,
}

impl CarrierServiceBridge {
    fn new(
        binary_path: String,
        env_vars: Vec<(String, String)>,
        init_config: serde_json::Value,
    ) -> Self {
        Self {
            init_config,
            io: Mutex::new(None),
            binary_path,
            env_vars,
            child: Mutex::new(None),
        }
    }

    fn spawn(&self) -> Result<ChildIo, ProviderError> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in &self.env_vars {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::Provider(format!(
                "failed to spawn carrier service '{}': {}",
                self.binary_path, e
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Provider("carrier service stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Provider("carrier service stdout unavailable".into()))?;

        if let Ok(mut guard) = self.child.lock() {
            *guard = Some(child);
        }

        Ok(ChildIo {
            reader: BufReader::new(stdout),
            writer: stdin,
        })
    }

    fn send_raw_blocking(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut guard = self
            .io
            .lock()
            .map_err(|_| ProviderError::Provider("carrier service bridge mutex poisoned".into()))?;

        if guard.is_none() {
            *guard = Some(self.spawn()?);
            let io = guard.as_mut().unwrap();
            // Send init message.
            let init_req = serde_json::json!({
                "op": "init",
                "config": self.init_config.clone()
            });
            let init_resp = Self::send_line_and_read_json(io, &init_req).map_err(|e| {
                *guard = None;
                ProviderError::Provider(format!("carrier service init failed: {e}"))
            })?;
            let init_ok = init_resp
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s == "ok")
                .unwrap_or(false);
            if !init_ok {
                *guard = None;
                return Err(ProviderError::Provider(format!(
                    "carrier service init rejected: {}",
                    init_resp
                )));
            }
            tracing::info!("carrier service '{}' initialized", self.binary_path);
        }

        let io = guard
            .as_mut()
            .ok_or_else(|| ProviderError::Provider("carrier service bridge unavailable".into()))?;

        match Self::send_line_and_read_json(io, request) {
            Ok(v) => Ok(v),
            Err(e) => {
                *guard = None;
                Err(e)
            }
        }
    }

    fn send_line_and_read_json(
        io: &mut ChildIo,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let payload = serde_json::to_string(request)
            .map_err(|e| ProviderError::Provider(format!("serialize failed: {e}")))?;
        io.writer
            .write_all(payload.as_bytes())
            .map_err(|e| ProviderError::Provider(format!("write failed: {e}")))?;
        io.writer
            .write_all(b"\n")
            .map_err(|e| ProviderError::Provider(format!("write newline failed: {e}")))?;
        io.writer
            .flush()
            .map_err(|e| ProviderError::Provider(format!("flush failed: {e}")))?;

        // Read response lines, skip non-JSON.
        let mut line = String::new();
        let deadline = std::time::Instant::now() + CARRIER_SERVICE_READ_TIMEOUT;
        for _ in 0..256 {
            if std::time::Instant::now() > deadline {
                return Err(ProviderError::Provider(
                    "timed out waiting for carrier service response".into(),
                ));
            }
            line.clear();
            let n = io
                .reader
                .read_line(&mut line)
                .map_err(|e| ProviderError::Provider(format!("read failed: {e}")))?;
            if n == 0 {
                return Err(ProviderError::Provider(
                    "carrier service closed stdout".into(),
                ));
            }
            if line.len() > 1_048_576 {
                return Err(ProviderError::Provider(
                    "carrier service response too large (max 1MB)".into(),
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                return Ok(v);
            }
        }
        Err(ProviderError::Provider(
            "did not receive JSON response from carrier service".into(),
        ))
    }
}

impl Drop for CarrierServiceBridge {
    fn drop(&mut self) {
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(ref mut child) = *child_guard {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Carrier-plane provider that runs a capsule binary directly on the host.
pub struct CarrierServiceProvider {
    scheme: &'static str,
    bridge: Arc<CarrierServiceBridge>,
}

impl CarrierServiceProvider {
    pub fn new(
        scheme: impl Into<String>,
        binary_path: String,
        env_vars: Vec<(String, String)>,
        init_config: serde_json::Value,
    ) -> Self {
        let scheme = scheme.into().to_ascii_lowercase();
        let scheme: &'static str = Box::leak(scheme.into_boxed_str());
        Self {
            scheme,
            bridge: Arc::new(CarrierServiceBridge::new(
                binary_path,
                env_vars,
                init_config,
            )),
        }
    }

    fn to_raw_request(request: &ResourceRequest) -> serde_json::Value {
        match request.action {
            ResourceAction::Read => serde_json::json!({
                "op": "read", "path": request.path, "token": "",
            }),
            ResourceAction::Write => serde_json::json!({
                "op": "write", "path": request.path, "token": "",
                "content": request.content.clone().unwrap_or_default(), "append": false,
            }),
            ResourceAction::Delete => serde_json::json!({
                "op": "delete", "path": request.path, "token": "",
                "recursive": request.recursive,
            }),
            ResourceAction::List => serde_json::json!({
                "op": "list", "path": request.path, "token": "",
            }),
            ResourceAction::Stat => serde_json::json!({
                "op": "stat", "path": request.path, "token": "",
            }),
            ResourceAction::Mkdir => serde_json::json!({
                "op": "mkdir", "path": request.path, "token": "",
                "parents": true,
            }),
            ResourceAction::Exists => serde_json::json!({
                "op": "exists", "path": request.path, "token": "",
            }),
        }
    }

    fn to_resource_response(
        action: ResourceAction,
        response: serde_json::Value,
    ) -> Result<ResourceResponse, ProviderError> {
        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        if status != "ok" {
            let code = response
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("provider_error");
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown provider error");
            if message.contains("not found") || message.contains("No such file") {
                return Err(ProviderError::NotFound(message.to_string()));
            }
            if message.contains("Permission denied") || message.contains("escapes") {
                return Err(ProviderError::PermissionDenied(message.to_string()));
            }
            return Err(ProviderError::Provider(format!("[{}] {}", code, message)));
        }

        let data = response.get("data").cloned();
        match action {
            ResourceAction::Read => {
                let data =
                    data.ok_or_else(|| ProviderError::Provider("read: missing data".into()))?;
                let content = data
                    .get("content")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| ProviderError::Provider("read: missing content".into()))?
                    .iter()
                    .map(|v| v.as_u64().unwrap_or(0) as u8)
                    .collect();
                Ok(ResourceResponse::Data(content))
            }
            ResourceAction::Write => {
                let bytes = data
                    .and_then(|d| d.get("bytes_written").and_then(|v| v.as_u64()))
                    .unwrap_or(0) as usize;
                Ok(ResourceResponse::Written { bytes })
            }
            ResourceAction::Delete => Ok(ResourceResponse::Deleted),
            ResourceAction::List => {
                let data =
                    data.ok_or_else(|| ProviderError::Provider("list: missing data".into()))?;
                let entries: Vec<serde_json::Value> = serde_json::from_value(data)
                    .map_err(|e| ProviderError::Provider(format!("parse list: {}", e)))?;
                let resource_entries = entries
                    .iter()
                    .map(|e| ResourceEntry {
                        name: e
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        is_directory: e.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false),
                        size: e.get("size").and_then(|v| v.as_u64()),
                        modified: None,
                    })
                    .collect();
                Ok(ResourceResponse::List(resource_entries))
            }
            ResourceAction::Stat => {
                let data =
                    data.ok_or_else(|| ProviderError::Provider("stat: missing data".into()))?;
                let is_dir = data
                    .get("is_dir")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let size = data.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let modified = data.get("modified").and_then(|v| v.as_u64()).unwrap_or(0);
                let entry_type = if is_dir {
                    EntryType::Directory
                } else {
                    EntryType::File
                };
                Ok(ResourceResponse::Metadata {
                    size,
                    entry_type,
                    modified,
                })
            }
            ResourceAction::Mkdir => Ok(ResourceResponse::Created),
            ResourceAction::Exists => {
                let exists = data
                    .and_then(|d| d.get("exists").and_then(|v| v.as_bool()))
                    .unwrap_or(false);
                Ok(ResourceResponse::Exists(exists))
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for CarrierServiceProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        let action = request.action;
        let raw_req = Self::to_raw_request(&request);
        let raw_resp = self.send_raw(&raw_req).await?;
        Self::to_resource_response(action, raw_resp)
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![self.scheme]
    }

    fn name(&self) -> &'static str {
        "carrier-service-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let bridge = Arc::clone(&self.bridge);
        let request = request.clone();
        tokio::task::spawn_blocking(move || bridge.send_raw_blocking(&request))
            .await
            .map_err(|e| {
                ProviderError::Provider(format!("carrier service bridge task join failed: {e}"))
            })?
    }
}
