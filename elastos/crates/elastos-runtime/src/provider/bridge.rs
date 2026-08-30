//! Provider bridge for capsule-based providers
//!
//! Manages stdin/stdout communication with a provider capsule process.
//! The runtime sends ProviderRequests and receives ProviderResponses
//! over line-delimited JSON.
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::registry::{
    EntryType, Provider, ProviderError, ResourceAction, ResourceEntry, ResourceRequest,
    ResourceResponse,
};

/// Timeout for provider requests (30 seconds)
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Timeout for provider init (10 seconds)
const INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Timeout for provider shutdown (5 seconds)
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// === Wire protocol types (mirror capsules/localhost-provider/src/main.rs) ===

/// Request from runtime to provider capsule
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderRequest {
    /// Initialize the provider
    Init { config: ProviderConfig },

    /// Read file contents
    Read {
        path: String,
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        length: Option<u64>,
    },

    /// Write file contents
    Write {
        path: String,
        token: String,
        content: Vec<u8>,
        #[serde(default)]
        append: bool,
    },

    /// List directory contents
    List { path: String, token: String },

    /// Delete file or directory
    Delete {
        path: String,
        token: String,
        #[serde(default)]
        recursive: bool,
    },

    /// Get file/directory metadata
    Stat { path: String, token: String },

    /// Create directory
    Mkdir {
        path: String,
        token: String,
        #[serde(default)]
        parents: bool,
    },

    /// Check if path exists
    Exists { path: String, token: String },

    /// Shutdown the provider
    Shutdown,
}

/// Response from provider capsule to runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderResponse {
    /// Operation succeeded
    Ok {
        #[serde(default)]
        data: Option<serde_json::Value>,
    },

    /// Operation failed
    Error { code: String, message: String },
}

/// Provider configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Base path for all operations (sandbox root)
    #[serde(default)]
    pub base_path: String,

    /// Allowed path prefixes (relative to base_path)
    #[serde(default)]
    pub allowed_paths: Vec<String>,

    /// Read-only mode
    #[serde(default)]
    pub read_only: bool,

    /// Hex-encoded AES-256 encryption key (empty = no encryption)
    #[serde(default)]
    pub encryption_key: String,

    /// Provider-specific configuration (passed through to provider init)
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

// === Bridge errors ===

/// Errors from provider bridge operations
#[derive(Debug)]
pub enum BridgeError {
    /// Failed to spawn provider process
    Spawn(std::io::Error),
    /// Provider initialization failed
    InitFailed(String),
    /// Request timed out
    Timeout,
    /// Provider process exited unexpectedly
    ProcessExited,
    /// Failed to serialize/deserialize
    Serde(serde_json::Error),
    /// I/O error
    Io(std::io::Error),
    /// Provider returned an error
    Provider { code: String, message: String },
    /// Background provider I/O task failed
    TaskJoin(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Spawn(e) => write!(f, "failed to spawn provider: {}", e),
            BridgeError::InitFailed(msg) => write!(f, "provider init failed: {}", msg),
            BridgeError::Timeout => write!(f, "provider request timed out"),
            BridgeError::ProcessExited => write!(f, "provider process exited unexpectedly"),
            BridgeError::Serde(e) => write!(f, "serialization error: {}", e),
            BridgeError::Io(e) => write!(f, "I/O error: {}", e),
            BridgeError::Provider { code, message } => {
                write!(f, "provider error [{}]: {}", code, message)
            }
            BridgeError::TaskJoin(message) => {
                write!(f, "provider bridge task failed: {}", message)
            }
        }
    }
}

impl std::error::Error for BridgeError {}

// === ProviderBridge ===

/// Internal I/O state for the bridge
struct ProviderIo {
    writer: Box<dyn AsyncWrite + Unpin + Send>,
    reader: Box<dyn AsyncBufRead + Unpin + Send>,
}

/// Bridge to a provider capsule process.
///
/// Manages serial request/response communication over stdin/stdout.
/// All requests are serialized through a mutex (the provider processes
/// them one at a time).
pub struct ProviderBridge {
    io: Arc<Mutex<ProviderIo>>,
    child: Mutex<Option<Child>>,
    /// True once a shutdown attempt has completed (child reaped, or the
    /// protocol shutdown was delivered on a childless bridge). Later
    /// shutdown() calls are idempotent no-ops.
    shutdown_completed: std::sync::atomic::AtomicBool,
    /// Timeout applied to each shutdown settle stage (protocol request,
    /// child wait, force reap). Tests inject a short value.
    shutdown_timeout: std::time::Duration,
}

impl ProviderBridge {
    async fn force_child_reap(
        child: &mut Child,
        shutdown_timeout: std::time::Duration,
    ) -> Result<(), BridgeError> {
        child.start_kill().map_err(BridgeError::Io)?;
        match tokio::time::timeout(shutdown_timeout, child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(BridgeError::Io(error)),
            Err(_) => Err(BridgeError::Timeout),
        }
    }

    async fn terminate_child_for_init_failure(
        child_mutex: &Mutex<Option<Child>>,
        shutdown_timeout: std::time::Duration,
    ) {
        let mut child_guard = child_mutex.lock().await;
        let Some(mut child) = child_guard.take() else {
            return;
        };

        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = Self::force_child_reap(&mut child, shutdown_timeout).await;
            }
        }
    }

    /// Spawn a provider capsule as a child process.
    ///
    /// Starts the binary, sends Init with the given config, and waits
    /// for the init response.
    pub async fn spawn(binary_path: &Path, config: ProviderConfig) -> Result<Self, BridgeError> {
        Self::spawn_with_timeouts(binary_path, config, INIT_TIMEOUT, SHUTDOWN_TIMEOUT).await
    }

    async fn spawn_with_timeouts(
        binary_path: &Path,
        config: ProviderConfig,
        init_timeout: std::time::Duration,
        shutdown_timeout: std::time::Duration,
    ) -> Result<Self, BridgeError> {
        let mut child = Command::new(binary_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(BridgeError::Spawn)?;

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let child = Mutex::new(Some(child));
                Self::terminate_child_for_init_failure(&child, shutdown_timeout).await;
                return Err(BridgeError::InitFailed(
                    "spawned provider missing piped stdin".to_string(),
                ));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let child = Mutex::new(Some(child));
                Self::terminate_child_for_init_failure(&child, shutdown_timeout).await;
                return Err(BridgeError::InitFailed(
                    "spawned provider missing piped stdout".to_string(),
                ));
            }
        };

        let bridge = Self {
            io: Arc::new(Mutex::new(ProviderIo {
                writer: Box::new(stdin),
                reader: Box::new(tokio::io::BufReader::new(stdout)),
            })),
            child: Mutex::new(Some(child)),
            shutdown_completed: std::sync::atomic::AtomicBool::new(false),
            shutdown_timeout,
        };

        // Send Init request
        let init_req = ProviderRequest::Init { config };
        let response = match tokio::time::timeout(init_timeout, bridge.request_raw(init_req)).await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let shutdown_error = bridge.shutdown().await.err().map(|err| err.to_string());
                let detail = match shutdown_error {
                    Some(shutdown_error) => {
                        format!("{error}; failed to settle provider child: {shutdown_error}")
                    }
                    None => error.to_string(),
                };
                return Err(BridgeError::InitFailed(detail));
            }
            Err(_) => {
                let shutdown_error = bridge.shutdown().await.err().map(|err| err.to_string());
                let detail = match shutdown_error {
                    Some(shutdown_error) => {
                        format!("provider init timed out; failed to settle provider child: {shutdown_error}")
                    }
                    None => "provider init timed out".to_string(),
                };
                return Err(BridgeError::InitFailed(detail));
            }
        };

        match response {
            ProviderResponse::Ok { .. } => Ok(bridge),
            ProviderResponse::Error { code, message } => {
                let shutdown_error = bridge.shutdown().await.err().map(|err| err.to_string());
                let detail = match shutdown_error {
                    Some(shutdown_error) => {
                        format!(
                            "{code}: {message}; failed to settle provider child: {shutdown_error}"
                        )
                    }
                    None => format!("{code}: {message}"),
                };
                Err(BridgeError::InitFailed(detail))
            }
        }
    }

    /// Create a bridge from existing I/O handles (for testing).
    pub fn from_io(
        reader: impl AsyncBufRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
        Self {
            io: Arc::new(Mutex::new(ProviderIo {
                writer: Box::new(writer),
                reader: Box::new(reader),
            })),
            child: Mutex::new(None),
            shutdown_completed: std::sync::atomic::AtomicBool::new(false),
            shutdown_timeout: SHUTDOWN_TIMEOUT,
        }
    }

    /// Send a request and receive a response (with timeout).
    pub async fn request(&self, req: ProviderRequest) -> Result<ProviderResponse, BridgeError> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_raw(req))
            .await
            .map_err(|_| BridgeError::Timeout)?
    }

    /// Send a request and receive a response (no timeout).
    async fn request_raw(&self, req: ProviderRequest) -> Result<ProviderResponse, BridgeError> {
        let line = self.send_json_line(req).await?;
        serde_json::from_str(line.trim()).map_err(BridgeError::Serde)
    }

    /// Send arbitrary JSON to the provider (bypasses typed ProviderRequest enum).
    /// Used by the generic provider proxy to forward custom ops.
    pub async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        let line = self.send_json_line(request.clone()).await?;
        serde_json::from_str(line.trim()).map_err(BridgeError::Serde)
    }

    /// Write one request and always drain exactly one response line.
    ///
    /// This runs in a detached task so caller cancellation cannot leave a stale
    /// provider response in the pipe for the next request. Without this, a
    /// cancelled HTTP request can cause the following provider call to receive
    /// the previous call's response, crossing authority/data boundaries.
    async fn send_json_line<T>(&self, request: T) -> Result<String, BridgeError>
    where
        T: Serialize + Send + 'static,
    {
        let io = Arc::clone(&self.io);
        tokio::spawn(async move {
            let mut io = io.lock().await;

            // Serialize and write request
            let json = serde_json::to_string(&request).map_err(BridgeError::Serde)?;
            io.writer
                .write_all(json.as_bytes())
                .await
                .map_err(BridgeError::Io)?;
            io.writer.write_all(b"\n").await.map_err(BridgeError::Io)?;
            io.writer.flush().await.map_err(BridgeError::Io)?;

            // Read response line
            let mut line = String::new();
            let n = io
                .reader
                .read_line(&mut line)
                .await
                .map_err(BridgeError::Io)?;

            if n == 0 {
                return Err(BridgeError::ProcessExited);
            }

            Ok(line)
        })
        .await
        .map_err(|err| BridgeError::TaskJoin(err.to_string()))?
    }

    /// Gracefully shut down the provider.
    pub async fn shutdown(&self) -> Result<(), BridgeError> {
        let mut child_guard = self.child.lock().await;
        let Some(child) = child_guard.as_mut() else {
            if self
                .shutdown_completed
                .load(std::sync::atomic::Ordering::Acquire)
            {
                // The child was already reaped (or the protocol shutdown was
                // already delivered): shutdown is idempotent.
                return Ok(());
            }
            // Never had a child process: the transport is the only handle on
            // the provider, so the protocol shutdown must still be delivered
            // for an attached provider to stop cleanly.
            let result = tokio::time::timeout(
                self.shutdown_timeout,
                self.request_raw(ProviderRequest::Shutdown),
            )
            .await
            .map_err(|_| BridgeError::Timeout)
            .and_then(|result| result)
            .and_then(|response| match response {
                ProviderResponse::Ok { .. } => Ok(()),
                ProviderResponse::Error { code, message } => {
                    Err(BridgeError::Provider { code, message })
                }
            });
            self.shutdown_completed
                .store(true, std::sync::atomic::Ordering::Release);
            return result;
        };

        let shutdown_result = tokio::time::timeout(
            self.shutdown_timeout,
            self.request_raw(ProviderRequest::Shutdown),
        )
        .await
        .map_err(|_| BridgeError::Timeout)
        .and_then(|result| result)
        .and_then(|response| match response {
            ProviderResponse::Ok { .. } => Ok(()),
            ProviderResponse::Error { code, message } => {
                Err(BridgeError::Provider { code, message })
            }
        });

        let protocol_error = shutdown_result.err();
        match tokio::time::timeout(self.shutdown_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                child_guard.take();
                self.shutdown_completed
                    .store(true, std::sync::atomic::Ordering::Release);
                if let Some(error) = protocol_error {
                    Err(error)
                } else if status.success() {
                    Ok(())
                } else {
                    Err(BridgeError::ProcessExited)
                }
            }
            Ok(Err(error)) => match Self::force_child_reap(child, self.shutdown_timeout).await {
                Ok(()) => {
                    child_guard.take();
                    self.shutdown_completed
                        .store(true, std::sync::atomic::Ordering::Release);
                    Err(protocol_error.unwrap_or(BridgeError::Io(error)))
                }
                Err(reap_error) => Err(reap_error),
            },
            Err(_) => match Self::force_child_reap(child, self.shutdown_timeout).await {
                Ok(()) => {
                    child_guard.take();
                    self.shutdown_completed
                        .store(true, std::sync::atomic::Ordering::Release);
                    Err(protocol_error.unwrap_or(BridgeError::Timeout))
                }
                Err(reap_error) => Err(reap_error),
            },
        }
    }
}

// === CapsuleProvider (implements Provider trait) ===

/// A provider that delegates to a capsule process via ProviderBridge.
pub struct CapsuleProvider {
    bridge: Arc<ProviderBridge>,
    /// Leaked once at construction — providers live for program lifetime.
    scheme_static: &'static str,
}

impl CapsuleProvider {
    /// Create a new CapsuleProvider wrapping a ProviderBridge.
    /// Defaults to the current first-party localhost provider scheme.
    pub fn new(bridge: Arc<ProviderBridge>) -> Self {
        Self::with_scheme(bridge, "localhost")
    }

    /// Create a new CapsuleProvider with a custom scheme name.
    pub fn with_scheme(bridge: Arc<ProviderBridge>, scheme: impl Into<String>) -> Self {
        let scheme_static = Box::leak(scheme.into().into_boxed_str()) as &'static str;
        Self {
            bridge,
            scheme_static,
        }
    }

    /// Get a reference to the underlying bridge for raw communication.
    pub fn bridge(&self) -> &Arc<ProviderBridge> {
        &self.bridge
    }

    /// Map a ResourceRequest to a ProviderRequest.
    fn to_provider_request(request: &ResourceRequest) -> ProviderRequest {
        // Runtime has already validated capabilities; provider trusts runtime
        let token = String::new();

        match request.action {
            ResourceAction::Read => ProviderRequest::Read {
                path: request.path.clone(),
                token,
                offset: None,
                length: None,
            },
            ResourceAction::Write => ProviderRequest::Write {
                path: request.path.clone(),
                token,
                content: request.content.clone().unwrap_or_default(),
                append: false,
            },
            ResourceAction::Delete => ProviderRequest::Delete {
                path: request.path.clone(),
                token,
                recursive: request.recursive,
            },
            ResourceAction::List => ProviderRequest::List {
                path: request.path.clone(),
                token,
            },
            ResourceAction::Stat => ProviderRequest::Stat {
                path: request.path.clone(),
                token,
            },
            ResourceAction::Mkdir => ProviderRequest::Mkdir {
                path: request.path.clone(),
                token,
                parents: true,
            },
            ResourceAction::Exists => ProviderRequest::Exists {
                path: request.path.clone(),
                token,
            },
        }
    }

    /// Map a ProviderResponse to a ResourceResponse, given the original action.
    fn to_resource_response(
        action: ResourceAction,
        response: ProviderResponse,
    ) -> Result<ResourceResponse, ProviderError> {
        match response {
            ProviderResponse::Error { code, message } => match code.as_str() {
                "read_failed" if message.contains("No such file") => {
                    Err(ProviderError::NotFound(message))
                }
                _ if message.contains("not found") || message.contains("No such file") => {
                    Err(ProviderError::NotFound(message))
                }
                _ if message.contains("Permission denied") || message.contains("escapes") => {
                    Err(ProviderError::PermissionDenied(message))
                }
                _ => Err(ProviderError::Provider(format!("[{}] {}", code, message))),
            },
            ProviderResponse::Ok { data } => match action {
                ResourceAction::Read => {
                    let data = data.ok_or_else(|| {
                        ProviderError::Provider("read response missing data".into())
                    })?;
                    let content = data
                        .get("content")
                        .ok_or_else(|| {
                            ProviderError::Provider("read response missing 'content'".into())
                        })?
                        .as_array()
                        .ok_or_else(|| ProviderError::Provider("'content' is not an array".into()))?
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
                    let data = data.ok_or_else(|| {
                        ProviderError::Provider("list response missing data".into())
                    })?;
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
                            is_directory: e
                                .get("is_dir")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                            size: e.get("size").and_then(|v| v.as_u64()),
                            modified: None,
                        })
                        .collect();
                    Ok(ResourceResponse::List(resource_entries))
                }
                ResourceAction::Stat => {
                    let data = data.ok_or_else(|| {
                        ProviderError::Provider("stat response missing data".into())
                    })?;
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
            },
        }
    }
}

#[async_trait::async_trait]
impl Provider for CapsuleProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        let action = request.action;
        let provider_req = Self::to_provider_request(&request);

        let response = self
            .bridge
            .request(provider_req)
            .await
            .map_err(|e| ProviderError::Provider(e.to_string()))?;

        Self::to_resource_response(action, response)
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![self.scheme_static]
    }

    fn name(&self) -> &'static str {
        "capsule-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.bridge
            .send_raw(request)
            .await
            .map_err(|e| ProviderError::Provider(e.to_string()))
    }

    async fn shutdown(&self) -> Result<(), ProviderError> {
        self.bridge
            .shutdown()
            .await
            .map_err(|error| ProviderError::Provider(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[cfg(unix)]
    fn write_provider_script(tempdir: &TempDir, name: &str, script: &str) -> std::path::PathBuf {
        let path = tempdir.path().join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    fn process_exists(pid: &str) -> bool {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid)
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn wait_for_file(path: &std::path::Path) {
        // Generous real-time budget: the marker is written by a freshly
        // spawned child process, which can be slow under machine load.
        for _ in 0..250 {
            if path.is_file() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("expected test provider marker {} to exist", path.display());
    }

    #[cfg(unix)]
    fn assert_process_absent(pid: &str) {
        if !process_exists(pid) {
            return;
        }
        for _ in 0..5 {
            if !process_exists(pid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("expected provider process {pid} to be terminated and reaped");
    }

    #[test]
    fn test_provider_request_serialization() {
        let req = ProviderRequest::Read {
            path: "test.txt".into(),
            token: "tok".into(),
            offset: None,
            length: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""op":"read""#));
        assert!(json.contains(r#""path":"test.txt""#));

        // Init
        let req = ProviderRequest::Init {
            config: ProviderConfig {
                base_path: "/tmp".into(),
                allowed_paths: vec!["*".into()],
                read_only: false,
                encryption_key: String::new(),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""op":"init""#));
        assert!(json.contains(r#""base_path":"/tmp""#));
    }

    #[test]
    fn test_provider_response_deserialization() {
        // Ok response
        let json = r#"{"status":"ok","data":{"content":[104,101,108,108,111],"size":5}}"#;
        let resp: ProviderResponse = serde_json::from_str(json).unwrap();
        assert!(matches!(resp, ProviderResponse::Ok { data: Some(_) }));

        // Error response
        let json = r#"{"status":"error","code":"read_failed","message":"No such file"}"#;
        let resp: ProviderResponse = serde_json::from_str(json).unwrap();
        assert!(matches!(resp, ProviderResponse::Error { .. }));
    }

    #[tokio::test]
    async fn test_bridge_request_response() {
        // Simulate a provider using DuplexStream
        let (client_read, mut server_write) = tokio::io::duplex(4096);
        let (mut server_read_raw, client_write) = tokio::io::duplex(4096);

        let bridge = ProviderBridge::from_io(tokio::io::BufReader::new(client_read), client_write);

        // Spawn a task to simulate the provider
        let server = tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(&mut server_read_raw);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();

            // Parse request
            let req: ProviderRequest = serde_json::from_str(line.trim()).unwrap();
            assert!(matches!(req, ProviderRequest::Read { .. }));

            // Send response
            let resp = ProviderResponse::Ok {
                data: Some(serde_json::json!({"content": [104, 101, 108, 108, 111], "size": 5})),
            };
            let json = serde_json::to_string(&resp).unwrap();
            server_write
                .write_all(format!("{}\n", json).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        // Send request through bridge
        let response = bridge
            .request(ProviderRequest::Read {
                path: "test.txt".into(),
                token: String::new(),
                offset: None,
                length: None,
            })
            .await
            .unwrap();

        match response {
            ProviderResponse::Ok { data: Some(d) } => {
                assert!(d.get("content").is_some());
            }
            _ => panic!("Expected Ok response with data"),
        }

        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_bridge_timeout() {
        // Create a bridge where the "provider" never responds
        let (client_read, _server_write) = tokio::io::duplex(4096);
        let (_server_read, client_write) = tokio::io::duplex(4096);

        let bridge = ProviderBridge::from_io(tokio::io::BufReader::new(client_read), client_write);

        // Override timeout to 100ms for testing
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            bridge.request_raw(ProviderRequest::Exists {
                path: "test".into(),
                token: String::new(),
            }),
        )
        .await;

        assert!(result.is_err()); // Timed out
    }

    #[tokio::test]
    async fn test_cancelled_raw_request_drains_response_before_next_request() {
        let (client_read, mut server_write) = tokio::io::duplex(4096);
        let (server_read_raw, client_write) = tokio::io::duplex(4096);
        let bridge = ProviderBridge::from_io(tokio::io::BufReader::new(client_read), client_write);

        let server = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let mut reader = tokio::io::BufReader::new(server_read_raw);

            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(first.trim()).unwrap()["op"],
                "first"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            server_write
                .write_all(b"{\"status\":\"ok\",\"data\":{\"op\":\"first\"}}\n")
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let mut second = String::new();
            reader.read_line(&mut second).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(second.trim()).unwrap()["op"],
                "second"
            );
            server_write
                .write_all(b"{\"status\":\"ok\",\"data\":{\"op\":\"second\"}}\n")
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let first = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            bridge.send_raw(&serde_json::json!({"op": "first"})),
        )
        .await;
        assert!(first.is_err());

        let second = bridge
            .send_raw(&serde_json::json!({"op": "second"}))
            .await
            .unwrap();
        assert_eq!(second["data"]["op"], "second");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_bridge_process_exit() {
        // Create a bridge where the "provider" closes immediately
        let (client_read, server_write) = tokio::io::duplex(4096);
        let (_server_read, client_write) = tokio::io::duplex(4096);

        // Drop server_write to simulate EOF
        drop(server_write);

        let bridge = ProviderBridge::from_io(tokio::io::BufReader::new(client_read), client_write);

        let result = bridge
            .request(ProviderRequest::Read {
                path: "test.txt".into(),
                token: String::new(),
                offset: None,
                length: None,
            })
            .await;

        assert!(matches!(result, Err(BridgeError::ProcessExited)));
    }

    #[cfg(unix)]
    fn write_mock_provider_script(
        root: &Path,
        init_response: &str,
        shutdown_response: Option<&str>,
        shutdown_exit: i32,
    ) -> (PathBuf, PathBuf) {
        let binary = root.join("mock-provider.sh");
        let pid_file = root.join("mock-provider.pid");
        let shutdown_response = shutdown_response.unwrap_or("");
        let script = format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" > '{}'\nIFS= read -r _init || exit 1\nprintf '%s\\n' '{}'\nIFS= read -r _shutdown || exit 0\nif [ -n '{}' ]; then printf '%s\\n' '{}'; fi\nexit {}\n",
            pid_file.display(),
            init_response,
            shutdown_response,
            shutdown_response,
            shutdown_exit
        );
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        (binary, pid_file)
    }

    #[cfg(unix)]
    fn read_pid(path: &Path) -> u32 {
        std::fs::read_to_string(path)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[cfg(unix)]
    fn process_is_running(pid: u32) -> bool {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid}"))
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Spawn a just-written mock provider script, tolerating Linux ETXTBSY:
    /// a concurrently forking test can briefly hold the script's write fd
    /// (fork duplicates open descriptors before exec closes them), and exec
    /// then fails with ExecutableFileBusy until that window closes.
    #[cfg(unix)]
    async fn spawn_test_bridge(
        binary: &std::path::Path,
        config: ProviderConfig,
    ) -> Result<ProviderBridge, BridgeError> {
        let mut attempts = 0u32;
        loop {
            match ProviderBridge::spawn(binary, config.clone()).await {
                Err(BridgeError::Spawn(error))
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy =>
                {
                    attempts += 1;
                    if attempts >= 40 {
                        return Err(BridgeError::Spawn(error));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                result => return result,
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_shutdown_requires_typed_ok_response() {
        let temp = tempfile::tempdir().unwrap();
        let (binary, pid_file) = write_mock_provider_script(
            temp.path(),
            r#"{"status":"ok"}"#,
            Some(r#"{"status":"error","code":"denied","message":"no"}"#),
            0,
        );
        let bridge = spawn_test_bridge(&binary, ProviderConfig::default())
            .await
            .unwrap();
        let pid = read_pid(&pid_file);
        let error = bridge.shutdown().await.unwrap_err();
        assert!(matches!(error, BridgeError::Provider { .. }));
        assert!(!process_is_running(pid));
        bridge.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_shutdown_requires_successful_exit_status() {
        let temp = tempfile::tempdir().unwrap();
        let (binary, pid_file) = write_mock_provider_script(
            temp.path(),
            r#"{"status":"ok"}"#,
            Some(r#"{"status":"ok"}"#),
            7,
        );
        let bridge = spawn_test_bridge(&binary, ProviderConfig::default())
            .await
            .unwrap();
        let pid = read_pid(&pid_file);
        let error = bridge.shutdown().await.unwrap_err();
        assert!(matches!(error, BridgeError::ProcessExited));
        assert!(!process_is_running(pid));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_shutdown_reaps_child_after_response_loss() {
        let temp = tempfile::tempdir().unwrap();
        let (binary, pid_file) =
            write_mock_provider_script(temp.path(), r#"{"status":"ok"}"#, None, 0);
        let bridge = spawn_test_bridge(&binary, ProviderConfig::default())
            .await
            .unwrap();
        let pid = read_pid(&pid_file);
        let error = bridge.shutdown().await.unwrap_err();
        assert!(matches!(error, BridgeError::ProcessExited));
        assert!(!process_is_running(pid));
        bridge.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_shutdown_kills_and_reaps_child_after_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("mock-provider-timeout.sh");
        let pid_file = temp.path().join("mock-provider-timeout.pid");
        let gate = temp.path().join("mock-provider-timeout.gate");
        let status = std::process::Command::new("mkfifo")
            .arg(&gate)
            .status()
            .unwrap();
        assert!(status.success());
        let script = format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" > '{}'\nIFS= read -r _init || exit 1\nprintf '%s\\n' '{}'\nIFS= read -r _shutdown || exit 0\ncat '{}' >/dev/null\n",
            pid_file.display(),
            r#"{"status":"ok"}"#,
            gate.display(),
        );
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();

        let bridge = spawn_test_bridge(&binary, ProviderConfig::default())
            .await
            .unwrap();
        let pid = read_pid(&pid_file);
        let error = bridge.shutdown().await.unwrap_err();
        assert!(matches!(error, BridgeError::Timeout));
        assert!(!process_is_running(pid));
        bridge.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_shutdown_is_idempotent_after_clean_exit() {
        let temp = tempfile::tempdir().unwrap();
        let (binary, pid_file) = write_mock_provider_script(
            temp.path(),
            r#"{"status":"ok"}"#,
            Some(r#"{"status":"ok"}"#),
            0,
        );
        let bridge = spawn_test_bridge(&binary, ProviderConfig::default())
            .await
            .unwrap();
        let pid = read_pid(&pid_file);
        bridge.shutdown().await.unwrap();
        assert!(!process_is_running(pid));
        bridge.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bridge_spawn_reaps_child_after_init_failure() {
        let temp = tempfile::tempdir().unwrap();
        let (binary, pid_file) = write_mock_provider_script(
            temp.path(),
            r#"{"status":"error","code":"invalid","message":"bad init"}"#,
            None,
            0,
        );
        let error = match spawn_test_bridge(&binary, ProviderConfig::default()).await {
            Ok(_) => panic!("expected init failure"),
            Err(error) => error,
        };
        assert!(matches!(error, BridgeError::InitFailed(_)));
        let pid = read_pid(&pid_file);
        assert!(!process_is_running(pid));
    }

    #[tokio::test]
    async fn test_spawn_reaps_provider_that_rejects_init_and_stays_alive() {
        let tempdir = TempDir::new().unwrap();
        let pid_path = tempdir.path().join("reject.pid");
        let script = format!(
            "#!/bin/sh\nprintf '%s' $$ > '{}'\nIFS= read -r _line || exit 0\nprintf '{{\"status\":\"error\",\"code\":\"invalid_config\",\"message\":\"reject\"}}\\n'\nwhile :; do :; done\n",
            pid_path.display()
        );
        let script_path = write_provider_script(&tempdir, "reject-provider.sh", &script);

        let error = match ProviderBridge::spawn(&script_path, ProviderConfig::default()).await {
            Ok(_) => panic!("init rejection should not return a usable bridge"),
            Err(error) => error,
        };

        assert!(matches!(error, BridgeError::InitFailed(_)));
        let pid = std::fs::read_to_string(&pid_path).unwrap();
        assert_process_absent(pid.trim());
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn test_spawn_reaps_provider_that_never_answers_init() {
        let tempdir = TempDir::new().unwrap();
        let pid_path = tempdir.path().join("hang.pid");
        // Consumes stdin without ever answering, so init times out and the
        // settle path must force-kill and reap the child. The detached
        // request task keeps the io lock, so the protocol shutdown can never
        // be delivered to a never-answering provider.
        let script = format!(
            "#!/bin/sh\nprintf '%s' $$ > '{}'\nwhile IFS= read -r _line; do :; done\n",
            pid_path.display()
        );
        let script_path = write_provider_script(&tempdir, "hung-provider.sh", &script);

        // The paused clock freezes the init timer, so the marker wait below
        // cannot race the init timeout no matter how loaded the machine is.
        let task = tokio::spawn(async move {
            ProviderBridge::spawn_with_timeouts(
                &script_path,
                ProviderConfig::default(),
                INIT_TIMEOUT,
                std::time::Duration::from_secs(1),
            )
            .await
        });
        tokio::task::yield_now().await;
        wait_for_file(&pid_path);
        // Fire the init timeout deterministically, then resume real time so
        // the settle stages race real child events (SIGKILL + reap) instead
        // of the auto-advancing paused clock.
        tokio::time::advance(INIT_TIMEOUT).await;
        tokio::time::resume();
        let error = match task.await.unwrap() {
            Ok(_) => panic!("init timeout should not return a usable bridge"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            BridgeError::InitFailed(ref detail) if detail.starts_with("provider init timed out")
        ));
        let pid = std::fs::read_to_string(&pid_path).unwrap();
        assert_process_absent(pid.trim());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_spawn_success_returns_usable_bridge() {
        let tempdir = TempDir::new().unwrap();
        let pid_path = tempdir.path().join("ok.pid");
        let script = format!(
            "#!/bin/sh\nprintf '%s' $$ > '{}'\nwhile IFS= read -r line; do\ncase \"$line\" in\n  *'\"op\":\"init\"'*) printf '{{\"status\":\"ok\"}}\\n' ;;\n  *'\"op\":\"exists\"'*) printf '{{\"status\":\"ok\",\"data\":{{\"exists\":true}}}}\\n' ;;\n  *'\"op\":\"shutdown\"'*) printf '{{\"status\":\"ok\"}}\\n'; exit 0 ;;\n  *) printf '{{\"status\":\"error\",\"code\":\"unexpected\",\"message\":\"unexpected\"}}\\n'; exit 0 ;;\nesac\ndone\n",
            pid_path.display()
        );
        let script_path = write_provider_script(&tempdir, "ok-provider.sh", &script);

        let bridge = ProviderBridge::spawn(&script_path, ProviderConfig::default())
            .await
            .unwrap();
        let response = bridge
            .request(ProviderRequest::Exists {
                path: "test".into(),
                token: String::new(),
            })
            .await
            .unwrap();
        assert!(matches!(response, ProviderResponse::Ok { .. }));
        bridge.shutdown().await.unwrap();

        let pid = std::fs::read_to_string(&pid_path).unwrap();
        assert_process_absent(pid.trim());
    }

    #[test]
    fn test_capsule_provider_request_mapping() {
        let request = ResourceRequest {
            uri: "localhost://Users/self/Documents/photos/test.jpg".into(),
            _scheme: "localhost".into(),
            path: "Users/self/Documents/photos/test.jpg".into(),
            _capsule_id: "cap-1".into(),
            action: ResourceAction::Read,
            content: None,
            recursive: false,
        };

        let provider_req = CapsuleProvider::to_provider_request(&request);
        match provider_req {
            ProviderRequest::Read { path, .. } => {
                assert_eq!(path, "Users/self/Documents/photos/test.jpg");
            }
            _ => panic!("Expected Read request"),
        }

        // Write mapping
        let request = ResourceRequest {
            uri: "localhost://Users/self/Documents/docs/file.txt".into(),
            _scheme: "localhost".into(),
            path: "Users/self/Documents/docs/file.txt".into(),
            _capsule_id: "cap-1".into(),
            action: ResourceAction::Write,
            content: Some(b"hello".to_vec()),
            recursive: false,
        };

        let provider_req = CapsuleProvider::to_provider_request(&request);
        match provider_req {
            ProviderRequest::Write { path, content, .. } => {
                assert_eq!(path, "Users/self/Documents/docs/file.txt");
                assert_eq!(content, b"hello");
            }
            _ => panic!("Expected Write request"),
        }
    }

    #[test]
    fn test_capsule_provider_response_mapping() {
        // Read response
        let resp = ProviderResponse::Ok {
            data: Some(serde_json::json!({"content": [104, 101, 108, 108, 111], "size": 5})),
        };
        let result = CapsuleProvider::to_resource_response(ResourceAction::Read, resp).unwrap();
        match result {
            ResourceResponse::Data(data) => assert_eq!(data, b"hello"),
            _ => panic!("Expected Data response"),
        }

        // List response
        let resp = ProviderResponse::Ok {
            data: Some(serde_json::json!([
                {"name": "file.txt", "is_file": true, "is_dir": false, "size": 11},
                {"name": "subdir", "is_file": false, "is_dir": true, "size": 0}
            ])),
        };
        let result = CapsuleProvider::to_resource_response(ResourceAction::List, resp).unwrap();
        match result {
            ResourceResponse::List(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].name, "file.txt");
                assert!(!entries[0].is_directory);
                assert_eq!(entries[1].name, "subdir");
                assert!(entries[1].is_directory);
            }
            _ => panic!("Expected List response"),
        }

        // Exists response
        let resp = ProviderResponse::Ok {
            data: Some(serde_json::json!({"exists": true})),
        };
        let result = CapsuleProvider::to_resource_response(ResourceAction::Exists, resp).unwrap();
        assert!(matches!(result, ResourceResponse::Exists(true)));

        // Error mapping (not found)
        let resp = ProviderResponse::Error {
            code: "read_failed".into(),
            message: "No such file or directory".into(),
        };
        let result = CapsuleProvider::to_resource_response(ResourceAction::Read, resp);
        assert!(matches!(result, Err(ProviderError::NotFound(_))));
    }
}
