//! Capsule I/O bridge for stdin/stdout communication
//!
//! Connects capsule stdio to the request handler:
//! - Reads JSON requests from capsule stdout
//! - Processes them via RequestHandler
//! - Writes JSON responses to capsule stdin

// Used by lib crate (tests, API handlers) but not directly by main.rs binary

use std::io::{BufRead, Write};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::capsule::CapsuleId;

use super::protocol::{RequestEnvelope, ResponseEnvelope, RuntimeRequest, RuntimeResponse};
use super::RequestHandler;

/// Maximum line length from a capsule (1 MB). Prevents OOM from malicious guests.
const MAX_LINE_BYTES: usize = 1_048_576;

/// I/O bridge for a single capsule
///
/// Manages bidirectional communication between a capsule and the runtime.
pub struct CapsuleIoBridge {
    /// Capsule ID
    capsule_id: CapsuleId,
    /// Request handler
    handler: Arc<RequestHandler>,
}

impl CapsuleIoBridge {
    /// Create a new I/O bridge for a capsule
    pub fn new(capsule_id: CapsuleId, handler: Arc<RequestHandler>) -> Self {
        Self {
            capsule_id,
            handler,
        }
    }

    /// Process a single request synchronously
    ///
    /// This is used for testing and simple synchronous execution.
    pub async fn process_request(&self, request: RuntimeRequest) -> RuntimeResponse {
        self.handler.handle(&self.capsule_id, request).await
    }

    /// Process a request envelope and return a response envelope
    pub async fn process_envelope(&self, envelope: RequestEnvelope) -> ResponseEnvelope {
        let response = self
            .handler
            .handle(&self.capsule_id, envelope.request)
            .await;
        ResponseEnvelope {
            id: envelope.id,
            response,
        }
    }

    /// Process a single line of JSON input
    ///
    /// Returns the JSON response string, or None if the input is invalid.
    /// Rejects lines exceeding MAX_LINE_BYTES to prevent OOM from malicious capsules.
    pub async fn process_line(&self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        if line.len() > MAX_LINE_BYTES {
            tracing::warn!(
                "Capsule {} sent oversized request ({} bytes, max {})",
                self.capsule_id,
                line.len(),
                MAX_LINE_BYTES
            );
            let error_response = ResponseEnvelope {
                id: 0,
                response: RuntimeResponse::error(
                    "request_too_large",
                    format!("Request exceeds maximum size of {} bytes", MAX_LINE_BYTES),
                ),
            };
            return serde_json::to_string(&error_response).ok();
        }

        // Parse request envelope
        let envelope: RequestEnvelope = match serde_json::from_str(line) {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!(
                    "Failed to parse request from capsule {}: {}",
                    self.capsule_id,
                    e
                );
                // Return an error response with id 0 (unknown)
                let error_response = ResponseEnvelope {
                    id: 0,
                    response: RuntimeResponse::error(
                        "parse_error",
                        format!("Failed to parse request: {}", e),
                    ),
                };
                return serde_json::to_string(&error_response).ok();
            }
        };

        // Process the request
        let response = self.process_envelope(envelope).await;

        // Serialize response
        match serde_json::to_string(&response) {
            Ok(json) => Some(json),
            Err(e) => {
                tracing::error!(
                    "Failed to serialize response for capsule {}: {}",
                    self.capsule_id,
                    e
                );
                None
            }
        }
    }

    /// Run the I/O bridge with async readers/writers
    ///
    /// This spawns tasks to handle bidirectional communication.
    pub async fn run_async<R, W>(
        self: Arc<Self>,
        reader: R,
        mut writer: W,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) where
        R: tokio::io::AsyncBufRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut lines = reader.lines();

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_rx.recv() => {
                    tracing::debug!("I/O bridge for {} received shutdown signal", self.capsule_id);
                    break;
                }

                // Read next line from capsule
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            if let Some(response_json) = self.process_line(&line).await {
                                // Write response back to capsule
                                if let Err(e) = writer.write_all(response_json.as_bytes()).await {
                                    tracing::error!(
                                        "Failed to write response to capsule {}: {}",
                                        self.capsule_id,
                                        e
                                    );
                                    break;
                                }
                                if let Err(e) = writer.write_all(b"\n").await {
                                    tracing::error!(
                                        "Failed to write newline to capsule {}: {}",
                                        self.capsule_id,
                                        e
                                    );
                                    break;
                                }
                                if let Err(e) = writer.flush().await {
                                    tracing::error!(
                                        "Failed to flush response to capsule {}: {}",
                                        self.capsule_id,
                                        e
                                    );
                                    break;
                                }
                            }
                        }
                        Ok(None) => {
                            // EOF - capsule closed stdout
                            tracing::debug!("Capsule {} closed stdout", self.capsule_id);
                            break;
                        }
                        Err(e) => {
                            tracing::error!(
                                "Error reading from capsule {}: {}",
                                self.capsule_id,
                                e
                            );
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!("I/O bridge for {} exiting", self.capsule_id);
    }

    /// Process multiple lines and return responses
    ///
    /// This is useful for testing with in-memory buffers.
    pub async fn process_lines<R, W>(&self, reader: R, mut writer: W)
    where
        R: BufRead,
        W: Write,
    {
        for line_result in reader.lines() {
            match line_result {
                Ok(line) => {
                    if let Some(response_json) = self.process_line(&line).await {
                        if writeln!(writer, "{}", response_json).is_err() {
                            break;
                        }
                        if writer.flush().is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Error reading from capsule {}: {}", self.capsule_id, e);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityManager, CapabilityStore};
    use crate::content::{ContentResolver, NullFetcher, ResolverConfig};
    use crate::messaging::MessageChannel;
    use crate::primitives::audit::AuditLog;
    use crate::primitives::metrics::MetricsManager;
    use elastos_common::{CapsuleManifest, CapsuleStatus, CapsuleType};
    use elastos_compute::{CapsuleHandle, CapsuleInfo as ComputeCapsuleInfo, ComputeProvider};
    use std::io::{BufReader, Cursor};
    use std::path::Path;

    // Mock compute provider
    struct MockComputeProvider;

    #[async_trait::async_trait]
    impl ComputeProvider for MockComputeProvider {
        fn supports(&self, _: &CapsuleType) -> bool {
            true
        }

        async fn load(
            &self,
            _: &Path,
            manifest: CapsuleManifest,
        ) -> elastos_common::Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: elastos_common::CapsuleId::new("test-handle"),
                manifest,
                args: vec![],
            })
        }

        async fn start(&self, _: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn stop(&self, _: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn status(&self, _: &CapsuleHandle) -> elastos_common::Result<CapsuleStatus> {
            Ok(CapsuleStatus::Running)
        }

        async fn info(&self, handle: &CapsuleHandle) -> elastos_common::Result<ComputeCapsuleInfo> {
            Ok(ComputeCapsuleInfo {
                id: handle.id.clone(),
                name: handle.manifest.name.clone(),
                status: CapsuleStatus::Running,
                memory_used_mb: 0,
            })
        }
    }

    async fn create_test_bridge() -> (CapsuleIoBridge, CapsuleId) {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());

        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        let capsule_manager = Arc::new(crate::capsule::CapsuleManager::new(
            compute,
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let message_channel = Arc::new(MessageChannel::new(
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let content_resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            audit_log.clone(),
            Arc::new(NullFetcher),
        ));

        let handler = Arc::new(RequestHandler::new(
            capsule_manager,
            capability_manager,
            message_channel,
            content_resolver,
            audit_log,
            "0.1.0".to_string(),
            None,
        ));

        let capsule_id = CapsuleId::new();
        handler.set_shell(capsule_id.clone()).await;

        (
            CapsuleIoBridge::new(capsule_id.clone(), handler),
            capsule_id,
        )
    }

    #[tokio::test]
    async fn test_process_ping() {
        let (bridge, _) = create_test_bridge().await;

        let response = bridge.process_request(RuntimeRequest::Ping).await;
        assert!(matches!(response, RuntimeResponse::Pong));
    }

    #[tokio::test]
    async fn test_process_envelope() {
        let (bridge, _) = create_test_bridge().await;

        let envelope = RequestEnvelope {
            id: 42,
            request: RuntimeRequest::Ping,
        };

        let response = bridge.process_envelope(envelope).await;
        assert_eq!(response.id, 42);
        assert!(matches!(response.response, RuntimeResponse::Pong));
    }

    #[tokio::test]
    async fn test_process_line() {
        let (bridge, _) = create_test_bridge().await;

        let input = r#"{"id":1,"request":{"type":"ping"}}"#;
        let output = bridge.process_line(input).await.unwrap();

        let response: ResponseEnvelope = serde_json::from_str(&output).unwrap();
        assert_eq!(response.id, 1);
        assert!(matches!(response.response, RuntimeResponse::Pong));
    }

    #[tokio::test]
    async fn test_process_invalid_json() {
        let (bridge, _) = create_test_bridge().await;

        let output = bridge.process_line("not valid json").await.unwrap();

        let response: ResponseEnvelope = serde_json::from_str(&output).unwrap();
        assert_eq!(response.id, 0);
        assert!(matches!(response.response, RuntimeResponse::Error { .. }));
    }

    #[tokio::test]
    async fn test_process_lines() {
        let (bridge, _) = create_test_bridge().await;

        let input = r#"{"id":1,"request":{"type":"ping"}}
{"id":2,"request":{"type":"get_runtime_info"}}
"#;
        let reader = Cursor::new(input);
        let mut output = Vec::new();

        bridge
            .process_lines(BufReader::new(reader), &mut output)
            .await;

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 2);

        let resp1: ResponseEnvelope = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(resp1.id, 1);
        assert!(matches!(resp1.response, RuntimeResponse::Pong));

        let resp2: ResponseEnvelope = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(resp2.id, 2);
        assert!(matches!(
            resp2.response,
            RuntimeResponse::RuntimeInfo { .. }
        ));
    }
}
