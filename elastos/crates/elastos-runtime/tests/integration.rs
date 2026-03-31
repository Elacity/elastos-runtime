//! End-to-end integration tests
//!
//! These tests exercise the full runtime flow:
//! - Runtime startup with shell bootstrap
//! - Message handling via I/O bridge
//! - Capability granting and usage
//! - Inter-capsule messaging
//! - Provider routing

// Import test utilities from the runtime crate
// Note: These tests run as integration tests, so they need to import from the crate

#[cfg(test)]
mod tests {
    use elastos_common::{CapsuleManifest, CapsuleStatus, CapsuleType};
    use elastos_compute::{CapsuleHandle, CapsuleInfo as ComputeCapsuleInfo, ComputeProvider};
    use elastos_runtime::bootstrap::{ElastosRuntime, RuntimeConfig, ShellConfig};
    use elastos_runtime::capability::{CapabilityManager, CapabilityStore};
    use elastos_runtime::capsule::CapsuleId;
    use elastos_runtime::capsule::CapsuleManager;
    use elastos_runtime::content::{ContentResolver, NullFetcher, ResolverConfig};
    use elastos_runtime::handler::{
        CapabilityConstraints, CapsuleIoBridge, RequestHandler, ResponseEnvelope, RuntimeRequest,
        RuntimeResponse,
    };
    use elastos_runtime::messaging::MessageChannel;
    use elastos_runtime::primitives::audit::AuditLog;
    use elastos_runtime::primitives::metrics::MetricsManager;
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceAction, ResourceResponse,
    };
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

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
                id: elastos_common::CapsuleId::new(format!("mock-{}", uuid::Uuid::new_v4())),
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

    /// Create a test runtime with all components
    async fn create_test_runtime() -> (
        Arc<CapsuleManager>,
        Arc<RequestHandler>,
        Arc<MessageChannel>,
        CapsuleId,
    ) {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());

        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        let capsule_manager = Arc::new(CapsuleManager::new(
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

        let request_handler = Arc::new(RequestHandler::new(
            capsule_manager.clone(),
            capability_manager,
            message_channel.clone(),
            content_resolver,
            audit_log,
            "0.1.0-test".to_string(),
            None,
        ));

        // Create and set shell ID
        let shell_id = CapsuleId::new();
        request_handler.set_shell(shell_id.clone()).await;
        message_channel.register(shell_id.as_str()).await;

        (capsule_manager, request_handler, message_channel, shell_id)
    }

    // ==================== I/O Bridge Tests ====================

    #[tokio::test]
    async fn test_io_bridge_ping_pong() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let bridge = CapsuleIoBridge::new(shell_id, request_handler);

        // Simulate capsule sending ping
        let input = r#"{"id":1,"request":{"type":"ping"}}"#;
        let output = bridge.process_line(input).await.unwrap();

        let response: ResponseEnvelope = serde_json::from_str(&output).unwrap();
        assert_eq!(response.id, 1);
        assert!(matches!(response.response, RuntimeResponse::Pong));
    }

    #[tokio::test]
    async fn test_io_bridge_get_runtime_info() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let bridge = CapsuleIoBridge::new(shell_id, request_handler);

        let input = r#"{"id":42,"request":{"type":"get_runtime_info"}}"#;
        let output = bridge.process_line(input).await.unwrap();

        let response: ResponseEnvelope = serde_json::from_str(&output).unwrap();
        assert_eq!(response.id, 42);

        match response.response {
            RuntimeResponse::RuntimeInfo {
                version,
                capsule_count,
            } => {
                assert_eq!(version, "0.1.0-test");
                assert_eq!(capsule_count, 0);
            }
            _ => panic!("Expected RuntimeInfo response"),
        }
    }

    #[tokio::test]
    async fn test_io_bridge_multi_line_processing() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let bridge = CapsuleIoBridge::new(shell_id, request_handler);

        // Simulate multiple requests
        let input = r#"{"id":1,"request":{"type":"ping"}}
{"id":2,"request":{"type":"get_runtime_info"}}
{"id":3,"request":{"type":"ping"}}
"#;
        let reader = std::io::Cursor::new(input);
        let mut output = Vec::new();

        bridge
            .process_lines(std::io::BufReader::new(reader), &mut output)
            .await;

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 3);

        // Verify each response
        for (i, line) in lines.iter().enumerate() {
            let response: ResponseEnvelope = serde_json::from_str(line).unwrap();
            assert_eq!(response.id, (i + 1) as u64);
        }
    }

    // ==================== Shell Authorization Tests ====================

    #[tokio::test]
    async fn test_shell_can_list_capsules() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let response = request_handler
            .handle(&shell_id, RuntimeRequest::ListCapsules)
            .await;

        match response {
            RuntimeResponse::CapsuleList { capsules } => {
                assert!(capsules.is_empty()); // No capsules launched yet
            }
            _ => panic!("Expected CapsuleList response"),
        }
    }

    #[tokio::test]
    async fn test_non_shell_cannot_list_capsules() {
        let (_, request_handler, _, _shell_id) = create_test_runtime().await;

        let other_capsule = CapsuleId::new();
        let response = request_handler
            .handle(&other_capsule, RuntimeRequest::ListCapsules)
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "unauthorized");
            }
            _ => panic!("Expected Error response"),
        }
    }

    #[tokio::test]
    async fn test_shell_can_grant_capability() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let response = request_handler
            .handle(
                &shell_id,
                RuntimeRequest::GrantCapability {
                    capsule_id: "target-capsule".to_string(),
                    resource: "localhost://Users/self/Documents/test/*".to_string(),
                    action: "read".to_string(),
                    constraints: CapabilityConstraints::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::CapabilityGranted { token_id } => {
                assert!(!token_id.is_empty());
                assert_eq!(token_id.len(), 32); // 16 bytes hex encoded
            }
            _ => panic!("Expected CapabilityGranted response"),
        }
    }

    #[tokio::test]
    async fn test_shell_can_revoke_capability() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        // First grant a capability
        let grant_response = request_handler
            .handle(
                &shell_id,
                RuntimeRequest::GrantCapability {
                    capsule_id: "target-capsule".to_string(),
                    resource: "localhost://Users/self/Documents/test/*".to_string(),
                    action: "read".to_string(),
                    constraints: CapabilityConstraints::default(),
                },
            )
            .await;

        let token_id = match grant_response {
            RuntimeResponse::CapabilityGranted { token_id } => token_id,
            _ => panic!("Expected CapabilityGranted"),
        };

        // Now revoke it
        let revoke_response = request_handler
            .handle(&shell_id, RuntimeRequest::RevokeCapability { token_id })
            .await;

        assert!(matches!(revoke_response, RuntimeResponse::Ok { .. }));
    }

    // ==================== Messaging Tests ====================

    #[tokio::test]
    async fn test_capsule_messaging() {
        let (_, request_handler, message_channel, shell_id) = create_test_runtime().await;

        // Register another capsule for messaging
        let capsule_b = CapsuleId::new();
        message_channel.register(capsule_b.as_str()).await;

        // Shell sends message to capsule_b
        let response = request_handler
            .handle(
                &shell_id,
                RuntimeRequest::SendMessage {
                    to: capsule_b.as_str().to_string(),
                    payload: b"hello from shell".to_vec(),
                    reply_to: None,
                    token: None,
                },
            )
            .await;

        assert!(matches!(response, RuntimeResponse::Ok { .. }));

        // Capsule B receives message
        // Note: We need to create a separate bridge for capsule_b to receive
        let bridge_b = CapsuleIoBridge::new(capsule_b.clone(), request_handler.clone());
        let recv_response = bridge_b
            .process_request(RuntimeRequest::ReceiveMessages)
            .await;

        match recv_response {
            RuntimeResponse::Messages { messages } => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].from, shell_id.as_str());
                assert_eq!(messages[0].payload, b"hello from shell");
            }
            _ => panic!("Expected Messages response"),
        }
    }

    // ==================== Provider Tests ====================

    /// In-memory mock provider for integration testing
    struct MockProvider {
        data: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn handle(
            &self,
            request: elastos_runtime::provider::ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            let mut data = self.data.lock().await;
            match request.action {
                ResourceAction::Read => data
                    .get(&request.path)
                    .cloned()
                    .map(ResourceResponse::Data)
                    .ok_or(ProviderError::NotFound(request.uri)),
                ResourceAction::Write => {
                    let content = request
                        .content
                        .ok_or_else(|| ProviderError::Provider("no content".into()))?;
                    let bytes = content.len();
                    data.insert(request.path, content);
                    Ok(ResourceResponse::Written { bytes })
                }
                _ => Err(ProviderError::Provider("unsupported".into())),
            }
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["http"]
        }

        fn name(&self) -> &'static str {
            "mock-http"
        }
    }

    #[tokio::test]
    async fn test_provider_registry_routing() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());

        registry.register(provider).await;

        // Write via registry
        let write_result = registry
            .route(
                "http://test/file.txt",
                "test-capsule",
                ResourceAction::Write,
                Some(b"test content".to_vec()),
            )
            .await;

        assert!(write_result.is_ok());

        // Read it back
        let read_result = registry
            .route(
                "http://test/file.txt",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await;

        match read_result.unwrap() {
            ResourceResponse::Data(data) => {
                assert_eq!(data, b"test content");
            }
            _ => panic!("Expected Data response"),
        }
    }

    // ==================== Full Runtime Lifecycle Tests ====================

    #[tokio::test]
    async fn test_runtime_start_stop() {
        let temp = tempdir().unwrap();
        let config = RuntimeConfig {
            data_dir: temp.path().to_path_buf(),
            enable_audit: false,
            shell: ShellConfig::default(),
            ..Default::default()
        };

        let compute = Arc::new(MockComputeProvider);
        let mut runtime = ElastosRuntime::build(config, compute, Arc::new(NullFetcher))
            .await
            .unwrap();

        // Start
        runtime.start().await.unwrap();
        assert!(runtime.is_running());

        // Can't start twice
        assert!(runtime.start().await.is_err());

        // Stop
        runtime.stop().await.unwrap();
        assert!(!runtime.is_running());

        // Can't stop twice
        assert!(runtime.stop().await.is_err());
    }

    #[tokio::test]
    async fn test_runtime_with_shell_config() {
        let temp = tempdir().unwrap();
        let config = RuntimeConfig {
            data_dir: temp.path().to_path_buf(),
            enable_audit: false,
            shell: ShellConfig::default(), // No shell configured = virtual shell
            ..Default::default()
        };

        let compute = Arc::new(MockComputeProvider);
        let mut runtime = ElastosRuntime::build(config, compute, Arc::new(NullFetcher))
            .await
            .unwrap();

        // Start should succeed (creates virtual shell)
        runtime.start().await.unwrap();
        assert!(runtime.is_running());

        // Request handler should have shell access
        let _handler = runtime.request_handler();
        let _shell_id = CapsuleId::new();

        // Since we used virtual shell, any capsule can be shell (in tests)
        // In production, only the configured shell would have access

        runtime.stop().await.unwrap();
    }

    // ==================== Error Handling Tests ====================

    #[tokio::test]
    async fn test_io_bridge_invalid_json() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let bridge = CapsuleIoBridge::new(shell_id, request_handler);

        let output = bridge.process_line("not valid json").await.unwrap();

        let response: ResponseEnvelope = serde_json::from_str(&output).unwrap();
        assert_eq!(response.id, 0); // Unknown request ID

        match response.response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "parse_error");
            }
            _ => panic!("Expected Error response"),
        }
    }

    #[tokio::test]
    async fn test_io_bridge_empty_line() {
        let (_, request_handler, _, shell_id) = create_test_runtime().await;

        let bridge = CapsuleIoBridge::new(shell_id, request_handler);

        // Empty line should return None
        let output = bridge.process_line("").await;
        assert!(output.is_none());

        // Whitespace only should also return None
        let output = bridge.process_line("   ").await;
        assert!(output.is_none());
    }
}
