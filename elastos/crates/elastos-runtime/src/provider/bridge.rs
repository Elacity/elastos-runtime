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
#[serde(tag = "op", rename_all = "snake_case")]
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
    io: Mutex<ProviderIo>,
    child: Mutex<Option<Child>>,
}

impl ProviderBridge {
    /// Spawn a provider capsule as a child process.
    ///
    /// Starts the binary, sends Init with the given config, and waits
    /// for the init response.
    pub async fn spawn(binary_path: &Path, config: ProviderConfig) -> Result<Self, BridgeError> {
        let mut child = Command::new(binary_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(BridgeError::Spawn)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            BridgeError::InitFailed("spawned provider missing piped stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BridgeError::InitFailed("spawned provider missing piped stdout".to_string())
        })?;

        let bridge = Self {
            io: Mutex::new(ProviderIo {
                writer: Box::new(stdin),
                reader: Box::new(tokio::io::BufReader::new(stdout)),
            }),
            child: Mutex::new(Some(child)),
        };

        // Send Init request
        let init_req = ProviderRequest::Init { config };
        let response = tokio::time::timeout(INIT_TIMEOUT, bridge.request_raw(init_req))
            .await
            .map_err(|_| BridgeError::Timeout)?
            .map_err(|e| BridgeError::InitFailed(e.to_string()))?;

        match response {
            ProviderResponse::Ok { .. } => Ok(bridge),
            ProviderResponse::Error { code, message } => {
                Err(BridgeError::InitFailed(format!("{}: {}", code, message)))
            }
        }
    }

    /// Create a bridge from existing I/O handles (for testing).
    pub fn from_io(
        reader: impl AsyncBufRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
        Self {
            io: Mutex::new(ProviderIo {
                writer: Box::new(writer),
                reader: Box::new(reader),
            }),
            child: Mutex::new(None),
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
        let mut io = self.io.lock().await;

        // Serialize and write request
        let json = serde_json::to_string(&req).map_err(BridgeError::Serde)?;
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

        serde_json::from_str(line.trim()).map_err(BridgeError::Serde)
    }

    /// Send arbitrary JSON to the provider (bypasses typed ProviderRequest enum).
    /// Used by the generic provider proxy to forward custom ops.
    pub async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        let mut io = self.io.lock().await;

        let json = serde_json::to_string(request).map_err(BridgeError::Serde)?;
        io.writer
            .write_all(json.as_bytes())
            .await
            .map_err(BridgeError::Io)?;
        io.writer.write_all(b"\n").await.map_err(BridgeError::Io)?;
        io.writer.flush().await.map_err(BridgeError::Io)?;

        let mut line = String::new();
        let n = io
            .reader
            .read_line(&mut line)
            .await
            .map_err(BridgeError::Io)?;

        if n == 0 {
            return Err(BridgeError::ProcessExited);
        }

        serde_json::from_str(line.trim()).map_err(BridgeError::Serde)
    }

    /// Gracefully shut down the provider.
    pub async fn shutdown(&self) -> Result<(), BridgeError> {
        // Send Shutdown request (ignore errors — provider may have already exited)
        let _ = tokio::time::timeout(
            SHUTDOWN_TIMEOUT,
            self.request_raw(ProviderRequest::Shutdown),
        )
        .await;

        // Wait for child to exit
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
                Ok(Ok(_)) => {}
                _ => {
                    // Force kill
                    let _ = child.kill().await;
                }
            }
        }

        Ok(())
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
    /// Defaults to "localhost" scheme for backwards compatibility.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
