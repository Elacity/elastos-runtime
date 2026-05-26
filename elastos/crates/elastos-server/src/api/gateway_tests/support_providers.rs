struct MockChainProvider;

#[async_trait::async_trait]
impl Provider for MockChainProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock chain provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["elastos"]
    }

    fn name(&self) -> &'static str {
        "mock-chain-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("networks") => Ok(json!({
                "status": "ok",
                "data": {
                    "networks": [
                        {
                            "id": "esc-mainnet",
                            "display_name": "Elastos Smart Chain",
                            "kind": "evm_json_rpc",
                            "chain_id": 20,
                            "native_symbol": "ELA",
                            "provider": "Elastos",
                            "mainnet": true,
                            "explorer_url": "https://esc.elastos.io"
                        }
                    ]
                }
            })),
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "chain_id_hex": "0x14",
                    "block_number_hex": "0x2a",
                    "block_number": 42
                }
            })),
            Some("sync_health") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.sync_health/v1",
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "syncing": false,
                    "healthy": true,
                    "latest_block": 42
                }
            })),
            Some("block_number") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "block_number_hex": "0x2a",
                    "block_number": 42
                }
            })),
            Some("node_lifecycle") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.node_lifecycle/v1",
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "managed": true,
                    "control_available": true,
                    "control_reason": "operator-approved supervisor configured",
                    "action": request
                        .get("action")
                        .and_then(|value| value.as_str())
                        .unwrap_or("status"),
                    "state": "managed_local",
                    "first_seen_at": 1,
                    "updated_at": 2
                }
            })),
            Some("balance") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "balance_hex": "0xde0b6b3a7640000",
                    "native_symbol": "ELA"
                }
            })),
            Some("contract_call") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.contract_call/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "to": required_test_str(request, "to")?,
                    "data": required_test_str(request, "data")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "result": "0x0000000000000000000000000000000000000000000000000000000000000042"
                }
            })),
            Some("estimate_gas") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.gas_estimate/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "from": required_test_str(request, "from")?,
                    "to": required_test_str(request, "to")?,
                    "value": request.get("value").and_then(|value| value.as_str()).unwrap_or("0x0"),
                    "data": request.get("data").and_then(|value| value.as_str()).unwrap_or("0x"),
                    "gas_limit": "0x5208"
                }
            })),
            Some("transaction_count") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.transaction_count/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("pending"),
                    "nonce": "0x7"
                }
            })),
            Some("gas_price") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.gas_price/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "gas_price": "0x3b9aca00"
                }
            })),
            Some("fee_history") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.fee_history/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "history": {
                        "oldestBlock": "0x1",
                        "baseFeePerGas": ["0x3b9aca00", "0x3b9aca01"],
                        "gasUsedRatio": [0.5],
                        "reward": [["0x1"]]
                    }
                }
            })),
            Some("code") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.code/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "code": "0x60016001"
                }
            })),
            Some("logs") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.logs/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "logs": [{
                        "address": "0x2222222222222222222222222222222222222222",
                        "blockNumber": "0x2a",
                        "data": "0x",
                        "topics": []
                    }]
                }
            })),
            Some("prepare_transaction") => {
                let network = required_test_str(request, "network")?;
                let chain_id = if network == "base-mainnet" { 8453 } else { 20 };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.unsigned_transaction_intent/v1",
                        "transaction_type": "eip155_legacy",
                        "network": {
                            "id": network,
                            "display_name": if network == "base-mainnet" { "Base" } else { "Elastos Smart Chain" },
                            "kind": "evm_json_rpc",
                            "chain_id": chain_id,
                            "native_symbol": if network == "base-mainnet" { "ETH" } else { "ELA" },
                            "provider": if network == "base-mainnet" { "Base" } else { "Elastos" },
                            "mainnet": true
                        },
                        "from": required_test_str(request, "from")?,
                        "to": required_test_str(request, "to")?,
                        "value": request.get("value").and_then(|value| value.as_str()).unwrap_or("0x0"),
                        "data": request.get("data").and_then(|value| value.as_str()).unwrap_or("0x"),
                        "chain_id": chain_id,
                        "nonce": "0x1",
                        "gas_price": "0x3b9aca00",
                        "gas_limit": "0x5208",
                        "requires_wallet_approval": true,
                        "wallet_intent": "transaction_intent"
                    }
                }))
            }
            Some("broadcast_transaction") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.broadcast_receipt/v1",
                    "network": required_test_str(request, "network")?,
                    "transaction_hash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            })),
            Some("erc1271_is_valid_signature") => {
                let signature = required_test_str(request, "signature")?;
                let signature_bytes = hex::decode(signature.trim_start_matches("0x"))
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let signature_hash =
                    format!("0x{}", hex::encode(sha2::Sha256::digest(signature_bytes)));
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.erc1271_proof/v1",
                        "network": {
                            "id": "esc-mainnet",
                            "display_name": "Elastos Smart Chain",
                            "kind": "evm_json_rpc",
                            "chain_id": 20,
                            "native_symbol": "ELA",
                            "provider": "Elastos",
                            "mainnet": true
                        },
                        "chain_id": 20,
                        "contract": required_test_str(request, "contract")?,
                        "message_hash": required_test_str(request, "message_hash")?,
                        "signature_hash": signature_hash,
                        "valid": true,
                        "magic_value": "0x1626ba7e",
                        "checked_at": crate::auth::now_ts()
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock chain op"
            })),
        }
    }
}

struct MockContentProvider;

#[async_trait::async_trait]
impl Provider for MockContentProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock content provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["content"]
    }

    fn name(&self) -> &'static str {
        "mock-content-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match (
            request.get("op").and_then(|value| value.as_str()),
            request.get("cid").and_then(|value| value.as_str()),
            request.get("path").and_then(|value| value.as_str()),
        ) {
            (Some("fetch"), Some(TEST_CIDV1), Some("index.html")) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "path": "index.html",
                    "data": base64::engine::general_purpose::STANDARD.encode(b"<html>content provider</html>"),
                    "availability": {
                        "status": "local_pinned",
                        "provider": "mock-content-provider",
                        "replicas": 1
                    }
                }
            })),
            _ => Ok(json!({
                "status": "error",
                "code": "not_found",
                "message": "mock content not found"
            })),
        }
    }
}

struct MockNetProvider;
struct MockMalformedNetProvider;

#[async_trait::async_trait]
impl Provider for MockNetProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock net provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["net"]
    }

    fn name(&self) -> &'static str {
        "mock-net"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("status") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "net-provider",
                    "status": "fail_closed",
                    "direct_network": false,
                    "operations": ["resolve", "connect", "stream", "http"],
                    "exit_count": 0
                }
            }));
        }
        let operation = request
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or("request");
        Ok(json!({
            "status": "error",
            "code": "exit_unavailable",
            "message": format!("No Browser Exit provider is configured for {operation}; net-provider refuses direct host networking")
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedNetProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed net provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["net"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-net"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "net-provider",
                "status": "exit_configured",
                "operations": ["resolve", "connect", "stream", "http"],
                "exit_count": 1
            }
        }))
    }
}

struct MockExitProvider;
struct MockMalformedExitProvider;

#[async_trait::async_trait]
impl Provider for MockExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("http_fetch") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.http-fetch.result/v1",
                    "backend": "mock-exit",
                    "url": request.get("url").cloned().unwrap_or_else(|| json!("")),
                    "method": request.get("method").cloned().unwrap_or_else(|| json!("GET")),
                    "body_text": "mock exit body",
                    "body_bytes": 14,
                    "body_truncated": false,
                    "status_code": 200
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("open_stream") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.stream-session/v1",
                    "backend": "mock-exit",
                    "stream_id": "stream:mock-exit:test",
                    "target": request.get("target").cloned().unwrap_or_else(|| json!("")),
                    "engine_owns_tls": true,
                    "state": "reserved",
                    "byte_transport": "not_attached"
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "fail_closed",
                "direct_network": false,
                "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                "backend_count": 0
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "backend_configured",
                "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                "backend_count": 1
            }
        }))
    }
}

struct MockAttachedExitProvider {
    relay_ipc_path: Option<String>,
    stream_id: String,
}

struct MockPolicyBlockedExitProvider;

#[async_trait::async_trait]
impl Provider for MockPolicyBlockedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock policy-blocked exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-policy-blocked-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        Ok(json!({
            "status": "error",
            "code": "exit_policy_blocked",
            "message": "No Browser Exit backend allows host whatismyip.com; exit-provider refuses direct host networking"
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockAttachedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock attached exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-attached-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("open_stream") {
            let stream_id = self.stream_id.clone();
            let relay_stream_id = stream_id.clone();
            let relay_ipc = self.relay_ipc_path.as_ref().map(|path| {
                json!({
                    "schema": "elastos.exit.relay-ipc/v1",
                    "kind": "unix_socket",
                    "path": path,
                    "stream_id": relay_stream_id
                })
            });
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.stream-session/v1",
                    "backend": "mock-attached-exit",
                    "stream_id": stream_id,
                    "target": request.get("target").cloned().unwrap_or_else(|| json!("")),
                    "scheme": "tls",
                    "host": "glidefinance.io",
                    "engine_owns_tls": true,
                    "state": "reserved",
                    "byte_transport": "adapter_ipc",
                    "adapter_ipc": {
                        "schema": "elastos.adapter-ipc/v1",
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock",
                        "stream_id": stream_id
                    },
                    "relay_ipc": relay_ipc
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "ready",
                "direct_network": false,
                "operations": ["open_stream"],
                "backend_count": 1
            }
        }))
    }
}

fn mock_attached_stream_id(cache_dir: &std::path::Path) -> String {
    let digest = sha2::Sha256::digest(cache_dir.to_string_lossy().as_bytes());
    format!("stream:mock-attached-exit:{}", hex::encode(&digest[..4]))
}

struct MockBrowserEngineProvider;
struct MockMalformedBrowserEngineProvider;

fn mock_browser_launch_page_id(request: &serde_json::Value) -> String {
    let url = request
        .get("url")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let reason = request
        .get("reason")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if url.is_empty() && reason.is_empty() {
        return "page:mock-browser-engine".to_string();
    }
    let digest = sha2::Sha256::digest(format!("{url}:{reason}").as_bytes());
    format!("page:mock-browser-engine-{}", hex::encode(&digest[..4]))
}

fn mock_browser_requested_page_id(request: &serde_json::Value) -> String {
    request
        .get("page_id")
        .and_then(|value| value.as_str())
        .unwrap_or("page:mock-browser-engine")
        .to_string()
}

fn mock_browser_frame_url(page_id: &str) -> String {
    format!(
        "/api/apps/browser/pages/{}/frame",
        page_id.replace(':', "%3A")
    )
}

#[async_trait::async_trait]
impl Provider for MockBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("launch") {
            let stream_session = request
                .get("stream_session")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if stream_session
                .get("byte_transport")
                .and_then(|value| value.as_str())
                != Some("adapter_ipc")
            {
                return Ok(json!({
                    "status": "error",
                    "code": "byte_transport_unavailable",
                    "message": "Browser Engine Adapter requires adapter_ipc"
                }));
            }
            assert_eq!(
                stream_session
                    .get("adapter_ipc")
                    .and_then(|value| value.get("schema"))
                    .and_then(|value| value.as_str()),
                Some("elastos.adapter-ipc/v1")
            );
            if let Some(relay_ipc) = stream_session.get("relay_ipc") {
                assert_eq!(
                    relay_ipc.get("schema").and_then(|value| value.as_str()),
                    Some("elastos.exit.relay-ipc/v1")
                );
                assert_eq!(
                    relay_ipc.get("kind").and_then(|value| value.as_str()),
                    Some("unix_socket")
                );
                assert!(relay_ipc
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .starts_with('/'));
            }
            let runtime_stream_path = stream_session
                .get("adapter_ipc")
                .and_then(|value| value.get("runtime_stream_path"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert!(runtime_stream_path.contains("elastos-browser-streams"));
            assert!(runtime_stream_path.ends_with(".sock"));
            let viewport = request
                .get("viewport")
                .cloned()
                .unwrap_or_else(|| json!({"width": 1280, "height": 720}));
            let display_mode = request
                .get("display_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("webrtc_remote_display");
            if display_mode != "diagnostic_frame" {
                return Ok(json!({
                    "status": "error",
                    "code": "display_session_unavailable",
                    "message": format!("{display_mode} display sessions are unavailable in the mock browser engine")
                }));
            }
            let page_id = mock_browser_launch_page_id(request);
            let frame_url = mock_browser_frame_url(&page_id);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.engine.page/v1",
                    "page_id": page_id,
                    "adapter": "mock-browser-engine",
                    "engine": "contract_proof",
                    "url": request.get("url").cloned().unwrap_or_else(|| json!("")),
                    "stream_id": stream_session.get("stream_id").cloned().unwrap_or_else(|| json!("")),
                    "network_mode": "runtime_net_only",
                    "direct_network": false,
                    "wallet_injection": false,
                    "display_session": {
                        "schema": "elastos.browser.display-session/v1",
                        "session_id": "display:mock-browser-engine",
                        "mode": "diagnostic_frame",
                        "network_mode": "runtime_net_only",
                        "direct_network": false,
                        "input": "runtime_route",
                        "audio": false,
                        "video": false
                    },
                    "view": {
                        "schema": "elastos.browser.view/v1",
                        "mode": "runtime_frame",
                        "width": viewport["width"],
                        "height": viewport["height"],
                        "frame_url": frame_url
                    }
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("screenshot") {
            let page_id = mock_browser_requested_page_id(request);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.screenshot/v1",
                    "page_id": page_id,
                    "content_type": "image/png",
                    "base64": base64::engine::general_purpose::STANDARD.encode(b"mock-png")
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("page_status") {
            let page_id = mock_browser_requested_page_id(request);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.page-status/v1",
                    "page_id": page_id,
                    "display_mode": "webrtc_remote_display",
                    "actual_url": "https://glidefinance.io/",
                    "frame_seq": 7,
                    "frame_count": 42,
                    "dropped_frames": 3,
                    "last_frame_age_ms": 25,
                    "last_frame_decode_ms": 6,
                    "last_frame_width": 1280,
                    "last_frame_height": 720,
                    "webrtc_connection_state": "connected",
                    "ice_connection_state": "connected",
                    "ice_gathering_state": "complete",
                    "direct_network": false
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("frame") {
            let page_id = mock_browser_requested_page_id(request);
            assert_eq!(
                request.get("since").and_then(|value| value.as_u64()),
                Some(1)
            );
            assert_eq!(
                request.get("wait_ms").and_then(|value| value.as_u64()),
                Some(25)
            );
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.frame/v1",
                    "page_id": page_id,
                    "seq": 2,
                    "changed": true,
                    "content_type": "image/png",
                    "base64": base64::engine::general_purpose::STANDARD.encode(b"mock-png-frame"),
                    "width": 900,
                    "height": 520,
                    "actual_url": "https://glidefinance.io/",
                    "title": "Glide"
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("input") {
            let page_id = mock_browser_requested_page_id(request);
            assert!(request.get("event").is_some());
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.input-result/v1",
                    "page_id": page_id,
                    "content_type": "image/png",
                    "screenshot": base64::engine::general_purpose::STANDARD.encode(b"mock-png-after-input")
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("close_page") {
            let page_id = mock_browser_requested_page_id(request);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.close-result/v1",
                    "page_id": page_id,
                    "closed": true
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("webrtc_signal") {
            let page_id = mock_browser_requested_page_id(request);
            let signal_schema = request
                .get("signal")
                .and_then(|value| value.get("schema"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert!(matches!(
                signal_schema,
                "elastos.browser.webrtc-offer/v1"
                    | "elastos.browser.webrtc-answer/v1"
                    | "elastos.browser.webrtc-candidate/v1"
                    | "elastos.browser.webrtc-end-of-candidates/v1"
            ));
            let signal_type = request
                .get("signal")
                .and_then(|value| value.get("type"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if signal_type == "answer"
                || signal_type == "candidate"
                || signal_type == "end_of_candidates"
            {
                return Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.browser.webrtc-signal-ack/v1",
                        "page_id": page_id,
                        "type": signal_type,
                        "accepted": true
                    }
                }));
            }
            assert_eq!(signal_type, "offer");
            if request
                .get("signal")
                .and_then(|value| value.get("sdp"))
                .and_then(|value| value.as_str())
                .is_some_and(|sdp| sdp.contains("simulate-provider-error"))
            {
                return Ok(json!({
                    "status": "error",
                    "code": "engine_process_unavailable",
                    "message": "browser page not found"
                }));
            }
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.webrtc-answer/v1",
                    "page_id": page_id,
                    "type": "answer",
                    "sdp": "v=0\r\ns=ElastOS Browser Test\r\n"
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "browser-engine-adapter",
                "status": "configured",
                "adapter_count": 1,
                "direct_network": false,
                "wallet_injection": false,
                "stream_session_schema": "elastos.exit.stream-session/v1",
                "required_byte_transport": "adapter_ipc",
                "display_session_schema": "elastos.browser.display-session/v1",
                "supported_display_modes": ["diagnostic_frame"],
                "operations": ["status", "launch", "attach_stream", "page_status", "screenshot", "frame", "input", "webrtc_signal", "close_page"]
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "browser-engine-adapter",
                "status": "configured",
                "adapter_count": 1,
                "required_byte_transport": "adapter_ipc",
                "stream_session_schema": "elastos.exit.stream-session/v1",
                "display_session_schema": "elastos.browser.display-session/v1",
                "supported_display_modes": ["webrtc_remote_display"]
            }
        }))
    }
}

#[derive(Default)]
struct MockWalletProvider {
    challenges: TokioMutex<HashMap<String, MockWalletChallenge>>,
    bitcoin_challenges: TokioMutex<HashMap<String, MockBitcoinChallenge>>,
    accounts: TokioMutex<Vec<serde_json::Value>>,
    approvals: TokioMutex<Vec<serde_json::Value>>,
    defaults: TokioMutex<Vec<serde_json::Value>>,
}

struct MockWalletChallenge {
    challenge: AuthChallengeV1,
    consumed: bool,
}

struct MockBitcoinChallenge {
    message: String,
    address: String,
    consumed: bool,
}

#[async_trait::async_trait]
impl Provider for MockWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock wallet provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "mock-wallet-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("challenge") => {
                let domain = required_test_str(request, "domain")?;
                let uri = required_test_str(request, "uri")?;
                let address = required_test_str(request, "address")?;
                let chain_id = request
                    .get("chain_id")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| ProviderError::Provider("missing chain_id".into()))?;
                let mut resources = vec![String::new()];
                resources.extend(required_test_string_array(request, "resources")?);
                let mut challenges = self.challenges.lock().await;
                let challenge_id = format!("wallet-test-{}", challenges.len() + 1);
                resources[0] = format!("elastos://auth/challenge/{challenge_id}");
                let challenge = AuthChallengeV1::new(AuthChallengeInput {
                    challenge_id: challenge_id.clone(),
                    domain: domain.to_string(),
                    uri: uri.to_string(),
                    address: address.to_string(),
                    chain_id,
                    nonce: format!("nonce{:08}", challenges.len() + 1),
                    issued_at: crate::auth::now_ts(),
                    ttl_secs: 300,
                    resources,
                });
                challenges.insert(
                    challenge_id.clone(),
                    MockWalletChallenge {
                        challenge: challenge.clone(),
                        consumed: false,
                    },
                );
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": AuthChallengeV1::SCHEMA,
                        "challenge_id": challenge_id,
                        "message": challenge.siwe_message(),
                        "expires_at": challenge.expires_at,
                        "resources": challenge.resources,
                    }
                }))
            }
            Some("bitcoin_challenge") => {
                let domain = required_test_str(request, "domain")?;
                let uri = required_test_str(request, "uri")?;
                let address = required_test_str(request, "address")?;
                let network = required_test_str(request, "network")?;
                let mut resources = vec![String::new()];
                resources.extend(required_test_string_array(request, "resources")?);
                let mut challenges = self.bitcoin_challenges.lock().await;
                let challenge_id = format!("bitcoin-test-{}", challenges.len() + 1);
                resources[0] = format!("elastos://auth/bitcoin-challenge/{challenge_id}");
                let now = crate::auth::now_ts();
                let message = format!(
                    "{domain} wants you to prove Bitcoin account ownership:\n{address}\n\nURI: {uri}\nVersion: 1\nNetwork: {network}\nNonce: bitcoin-nonce\nIssued At: {now}\nExpiration Time: {expires_at}\nResources:\n{resources}",
                    expires_at = now + 300,
                    resources = resources
                        .iter()
                        .map(|resource| format!("- {resource}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                challenges.insert(
                    challenge_id.clone(),
                    MockBitcoinChallenge {
                        message: message.clone(),
                        address: address.to_string(),
                        consumed: false,
                    },
                );
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.bitcoin_challenge/v1",
                        "challenge_id": challenge_id,
                        "message": message,
                        "expires_at": now + 300,
                        "network": network,
                        "address": address,
                        "resources": resources,
                        "proof_type": "bip322_simple",
                    }
                }))
            }
            Some("verify_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let parsed = elastos_runtime::auth::parse_siwe_message(message)
                    .map_err(ProviderError::Provider)?;
                let challenge_id = parsed
                    .resources
                    .iter()
                    .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
                    .ok_or_else(|| ProviderError::Provider("missing challenge resource".into()))?;
                let mut challenges = self.challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "challenge already consumed"
                    }));
                }
                let proof = match verify_siwe_challenge(
                    &stored.challenge,
                    message,
                    signature,
                    crate::auth::now_ts(),
                ) {
                    Ok(proof) => proof,
                    Err(err) => {
                        return Ok(json!({
                            "status": "error",
                            "code": "invalid_proof",
                            "message": err
                        }));
                    }
                };
                stored.consumed = true;
                let proof_binding_id = proof.binding.id();
                let chain_id = proof.binding.chain_id.unwrap_or_default();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": proof_binding_id,
                        "chain_namespace": format!("eip155:{chain_id}"),
                        "address": proof.recovered_address,
                        "proof_type": "siwe",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": format!("0x{}", hex::encode(proof.message_hash)),
                    }
                }))
            }
            Some("verify_bip322_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let challenge_id = message
                    .lines()
                    .find_map(|line| {
                        line.trim()
                            .strip_prefix("- elastos://auth/bitcoin-challenge/")
                    })
                    .ok_or_else(|| ProviderError::Provider("missing Bitcoin challenge".into()))?;
                let mut challenges = self.bitcoin_challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("Bitcoin challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "Bitcoin challenge already consumed"
                    }));
                }
                if message != stored.message {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "Bitcoin challenge message does not match"
                    }));
                }
                if signature != "mock-bip322-signature" {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_bip322_proof",
                        "message": "invalid mock BIP-322 signature"
                    }));
                }
                stored.consumed = true;
                let chain_namespace = "bip122:000000000019d6689c085ae165831e93";
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": format!("proof:wallet:{chain_namespace}:{}", stored.address),
                        "chain_namespace": chain_namespace,
                        "address": stored.address,
                        "proof_type": "bip322_simple",
                        "proof_strength": "standard",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": "0x010203",
                    }
                }))
            }
            Some("verify_contract_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let proof = request
                    .get("erc1271_proof")
                    .ok_or_else(|| ProviderError::Provider("missing erc1271_proof".into()))?;
                let parsed = elastos_runtime::auth::parse_siwe_message(message)
                    .map_err(ProviderError::Provider)?;
                let challenge_id = parsed
                    .resources
                    .iter()
                    .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
                    .ok_or_else(|| ProviderError::Provider("missing challenge resource".into()))?;
                let mut challenges = self.challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "challenge already consumed"
                    }));
                }
                if message != stored.challenge.siwe_message() {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "SIWE message does not match challenge"
                    }));
                }
                let message_hash = ethereum_signed_message_hash(message.as_bytes());
                let expected_message_hash = format!("0x{}", hex::encode(message_hash));
                let signature_bytes = hex::decode(signature.trim_start_matches("0x"))
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let expected_signature_hash =
                    format!("0x{}", hex::encode(sha2::Sha256::digest(signature_bytes)));
                if proof.get("valid").and_then(|value| value.as_bool()) != Some(true)
                    || proof.get("chain_id").and_then(|value| value.as_u64())
                        != Some(parsed.chain_id)
                    || proof.get("contract").and_then(|value| value.as_str())
                        != Some(parsed.address.as_str())
                    || proof.get("message_hash").and_then(|value| value.as_str())
                        != Some(expected_message_hash.as_str())
                    || proof.get("signature_hash").and_then(|value| value.as_str())
                        != Some(expected_signature_hash.as_str())
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_contract_proof",
                        "message": "ERC-1271 proof mismatch"
                    }));
                }
                stored.consumed = true;
                let proof_binding_id = ProofBinding::evm_account(
                    parsed.chain_id,
                    &parsed.address,
                    crate::auth::now_ts(),
                )
                .id();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": proof_binding_id,
                        "chain_namespace": format!("eip155:{}", parsed.chain_id),
                        "address": parsed.address,
                        "proof_type": "siwe_erc1271",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": expected_message_hash,
                    }
                }))
            }
            Some("link_account") => {
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let address = required_test_str(request, "address")?;
                let connector_id = required_test_str(request, "connector_id")?;
                let account = json!({
                    "account_id": format!("wallet:{chain_namespace}:{address}"),
                    "principal_id": required_test_str(request, "principal_id")?,
                    "proof_binding_id": required_test_str(request, "proof_binding_id")?,
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": required_test_str(request, "proof_type")?,
                    "connector_id": connector_id,
                    "linked_at": crate::auth::now_ts()
                });
                let mut accounts = self.accounts.lock().await;
                accounts.push(account.clone());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account }
                }))
            }
            Some("accounts") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let accounts = self.accounts.lock().await;
                let visible = accounts
                    .iter()
                    .filter(|account| {
                        account.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                            && account.get("revoked_at").is_none()
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let defaults = self.defaults.lock().await;
                let visible_defaults = defaults
                    .iter()
                    .filter(|default| {
                        default.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "accounts": visible,
                        "default_accounts": visible_defaults
                    }
                }))
            }
            Some("set_default_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let intent = required_test_str(request, "intent")?;
                let account_id = required_test_str(request, "account_id")?;
                let accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                if account
                    .get("chain_namespace")
                    .and_then(|value| value.as_str())
                    != Some(chain_namespace)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "default wallet chain must match the linked account"
                    }));
                }
                drop(accounts);
                let default_account = json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": principal_id,
                    "chain_namespace": chain_namespace,
                    "intent": intent,
                    "account_id": account_id,
                    "set_at": crate::auth::now_ts()
                });
                let mut defaults = self.defaults.lock().await;
                if let Some(existing) = defaults.iter_mut().find(|existing| {
                    existing
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && existing
                            .get("chain_namespace")
                            .and_then(|value| value.as_str())
                            == Some(chain_namespace)
                        && existing.get("intent").and_then(|value| value.as_str()) == Some(intent)
                }) {
                    *existing = default_account.clone();
                } else {
                    defaults.push(default_account.clone());
                }
                Ok(json!({
                    "status": "ok",
                    "data": { "default_account": default_account }
                }))
            }
            Some("create_managed_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let create_new = request
                    .get("create_new")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let (_base_address, proof_type) =
                    if chain_namespace == "bip122:000000000019d6689c085ae165831e93" {
                        (
                            "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                            "managed_btc_p2wpkh",
                        )
                    } else {
                        ("0x1111111111111111111111111111111111111111", "managed_evm")
                    };
                let mut accounts = self.accounts.lock().await;
                if !create_new {
                    if let Some(account) = accounts.iter().find(|account| {
                        account.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                            && account
                                .get("chain_namespace")
                                .and_then(|value| value.as_str())
                                == Some(chain_namespace)
                            && account.get("proof_type").and_then(|value| value.as_str())
                                == Some(proof_type)
                    }) {
                        return Ok(json!({
                            "status": "ok",
                            "data": { "account": account, "created": false }
                        }));
                    }
                }
                let address = if chain_namespace == "bip122:000000000019d6689c085ae165831e93" {
                    if create_new {
                        format!(
                            "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx{:02}",
                            accounts.len()
                        )
                    } else {
                        "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l".to_string()
                    }
                } else if create_new {
                    format!("0x{:040x}", accounts.len() + 1)
                } else {
                    "0x1111111111111111111111111111111111111111".to_string()
                };
                let account = json!({
                    "account_id": format!("wallet:{chain_namespace}:{address}"),
                    "principal_id": principal_id,
                    "proof_binding_id": format!("proof:wallet:managed:{chain_namespace}:{address}"),
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": proof_type,
                    "signing_available": true,
                    "signing_status": "managed_key_available",
                    "label": request.get("label").and_then(|value| value.as_str()).unwrap_or("Managed"),
                    "linked_at": crate::auth::now_ts()
                });
                accounts.push(account.clone());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account, "created": true }
                }))
            }
            Some("revoke_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let mut accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "linked account not found"
                    }));
                };
                account["revoked_at"] = json!(crate::auth::now_ts());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account.clone() }
                }))
            }
            Some("rename_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let label = required_test_str(request, "label")?.trim().to_string();
                if label.is_empty() {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "label is required"
                    }));
                }
                let mut accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                        && account.get("revoked_at").is_none()
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                account["label"] = json!(label);
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account.clone() }
                }))
            }
            Some("export_managed_secret") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                        && account.get("revoked_at").is_none()
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                if !account
                    .get("proof_type")
                    .and_then(|value| value.as_str())
                    .is_some_and(|proof| proof == "managed_evm" || proof == "managed_btc_p2wpkh")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "external_wallet_required",
                        "message": "recovery key is available only for passkey-managed accounts"
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.recovery-key/v1",
                        "account_id": account_id,
                        "chain_namespace": account["chain_namespace"],
                        "address": account["address"],
                        "secret_type": "secp256k1_private_key_hex",
                        "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111",
                        "note": "This account was created as an encrypted signing key, not a BIP39 seed phrase."
                    }
                }))
            }
            Some("import_managed_secret") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let Some(recovery_key) = request.get("recovery_key") else {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "recovery_key is required"
                    }));
                };
                if recovery_key
                    .get("schema")
                    .and_then(|value| value.as_str())
                    != Some("elastos.wallet.recovery-key/v1")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "expected elastos.wallet.recovery-key/v1"
                    }));
                }
                let account_id = required_test_str(recovery_key, "account_id")?;
                let chain_namespace = required_test_str(recovery_key, "chain_namespace")?;
                let address = required_test_str(recovery_key, "address")?;
                let proof_type = if chain_namespace
                    == "bip122:000000000019d6689c085ae165831e93"
                {
                    "managed_btc_p2wpkh"
                } else {
                    "managed_evm"
                };
                let label = request
                    .get("label")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Imported");
                let imported = json!({
                    "account_id": account_id,
                    "principal_id": principal_id,
                    "proof_binding_id": format!("proof:wallet:managed:{chain_namespace}:{address}"),
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": proof_type,
                    "signing_available": true,
                    "signing_status": "managed_key_available",
                    "label": label,
                    "linked_at": crate::auth::now_ts()
                });
                let mut accounts = self.accounts.lock().await;
                if let Some(existing) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) {
                    *existing = imported.clone();
                } else {
                    accounts.push(imported.clone());
                }
                Ok(json!({
                    "status": "ok",
                    "data": { "account": imported, "imported": true }
                }))
            }
            Some("approval_requests") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let include_resolved = request
                    .get("include_resolved")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let approvals = self.approvals.lock().await;
                let approval_requests = approvals
                    .iter()
                    .filter(|approval| {
                        approval
                            .get("principal_id")
                            .and_then(|value| value.as_str())
                            == Some(principal_id)
                    })
                    .filter(|approval| {
                        include_resolved
                            || approval.get("status").and_then(|value| value.as_str())
                                == Some("pending")
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_requests": approval_requests }
                }))
            }
            Some("request_signature") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let intent = required_test_str(request, "intent")?;
                let accounts = self.accounts.lock().await;
                let account_id = match request.get("account_id").and_then(|value| value.as_str()) {
                    Some(account_id) => account_id.to_string(),
                    None => {
                        let defaults = self.defaults.lock().await;
                        let Some(default) = defaults.iter().find(|default| {
                            default.get("principal_id").and_then(|value| value.as_str())
                                == Some(principal_id)
                                && default
                                    .get("chain_namespace")
                                    .and_then(|value| value.as_str())
                                    == Some(chain_namespace)
                                && default.get("intent").and_then(|value| value.as_str())
                                    == Some(intent)
                        }) else {
                            return Ok(json!({
                                "status": "error",
                                "code": "not_found",
                                "message": "default linked account not set"
                            }));
                        };
                        default
                            .get("account_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string()
                    }
                };
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id.as_str())
                        && account
                            .get("chain_namespace")
                            .and_then(|value| value.as_str())
                            == Some(chain_namespace)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                let account = account.clone();
                let payload = request.get("payload").cloned().unwrap_or_else(|| json!({}));
                let payload_bytes = serde_json::to_vec(&payload)
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let payload_hash = format!("0x{}", hex::encode(Keccak256::digest(&payload_bytes)));
                drop(accounts);
                let mut approvals = self.approvals.lock().await;
                let request_id = format!("wallet-approval:mock-{}", approvals.len() + 1);
                let approval = json!({
                    "request_id": request_id,
                    "status": "pending",
                    "intent": intent,
                    "capsule_id": required_test_str(request, "capsule_id")?,
                    "resource": required_test_str(request, "resource")?,
                    "reason": required_test_str(request, "reason")?,
                    "account_id": account_id,
                    "chain_namespace": chain_namespace,
                    "address": account.get("address").cloned().unwrap_or(json!("0x0")),
                    "proof_type": account.get("proof_type").cloned().unwrap_or(json!("siwe")),
                    "connector_id": account.get("connector_id").cloned().unwrap_or(json!(null)),
                    "payload_hash": payload_hash,
                    "principal_id": principal_id,
                    "created_at": crate::auth::now_ts(),
                    "expires_at": crate::auth::now_ts() + 600
                });
                approvals.push(approval.clone());
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval,
                        "requires_approval": true,
                        "signature": serde_json::Value::Null
                    }
                }))
            }
            Some("reject_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                approval["status"] = serde_json::Value::String("rejected".to_string());
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_request": approval.clone() }
                }))
            }
            Some("approve_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                approval["status"] = serde_json::Value::String("approved".to_string());
                let payload_hash = approval
                    .get("payload_hash")
                    .and_then(|value| value.as_str())
                    .unwrap_or(
                        "0x0000000000000000000000000000000000000000000000000000000000000000",
                    );
                let signer = approval
                    .get("address")
                    .and_then(|value| value.as_str())
                    .unwrap_or("0xabc");
                let handoff = if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    json!({
                        "schema": "elastos.wallet.webconnect_handoff/v1",
                        "request_id": request_id,
                        "intent": approval.get("intent").cloned().unwrap_or(json!("transaction_intent")),
                        "payload_hash": payload_hash,
                        "signer": signer,
                        "transaction": {
                            "from": signer,
                            "to": "0x2222222222222222222222222222222222222222",
                            "value": "0x1",
                            "data": "0x",
                            "gas": "0x5208",
                            "gasPrice": "0x3b9aca00",
                            "nonce": "0x1",
                            "chainId": "0x14"
                        },
                        "status": "awaiting_wallet_transaction"
                    })
                } else {
                    json!({
                        "schema": "elastos.wallet.webconnect_handoff/v1",
                        "request_id": request_id,
                        "intent": approval.get("intent").cloned().unwrap_or(json!("publish_envelope")),
                        "payload_hash": payload_hash,
                        "signer": signer,
                        "message": format!("ElastOS Wallet Approval\n\nRequest: {request_id}"),
                        "status": "awaiting_wallet_signature"
                    })
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval.clone(),
                        "handoff": handoff,
                        "signature": serde_json::Value::Null
                    }
                }))
            }
            Some("complete_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let connector_id = required_test_str(request, "connector_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval
                    .get("connector_id")
                    .and_then(|value| value.as_str())
                    != Some(connector_id)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request belongs to a different connector"
                    }));
                }
                approval["status"] = serde_json::Value::String("completed".to_string());
                if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    approval["signed_result"] = json!({
                        "schema": "elastos.wallet.external-transaction-result/v1",
                        "request_id": request_id,
                        "method": "eth_sendTransaction",
                        "transaction_hash": required_test_str(request, "transaction_hash")?,
                        "signer": required_test_str(request, "signer")?,
                        "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:20")),
                        "payload_hash": required_test_str(request, "payload_hash")?,
                    });
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval.clone(),
                        "signature_receipt": {
                            "schema": "elastos.wallet.signature_receipt/v1",
                            "request_id": request_id,
                            "signer": required_test_str(request, "signer")?,
                            "payload_hash": required_test_str(request, "payload_hash")?,
                            "signature_hash": "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                            "completed_at": crate::auth::now_ts()
                        }
                    }
                }))
            }
            Some("sign_approved") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval.get("status").and_then(|value| value.as_str()) != Some("approved") {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request must be approved before managed signing"
                    }));
                }
                approval["status"] = serde_json::Value::String("completed".to_string());
                approval["signature_receipt"] = json!({
                    "schema": "elastos.wallet.signature_receipt/v1",
                    "request_id": request_id,
                    "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                    "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    "signature_hash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "completed_at": crate::auth::now_ts(),
                });
                if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    approval["signed_result"] = json!({
                        "schema": "elastos.wallet.managed-transaction-result/v1",
                        "request_id": request_id,
                        "method": "eth_sendRawTransaction",
                        "signed_transaction": "0x1234",
                        "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                        "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:20")),
                        "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    });
                }
                let mut data = json!({
                    "approval_request": approval.clone(),
                    "signature_receipt": approval["signature_receipt"],
                    "signature": "0xsigned-managed",
                    "signed_payload": {}
                });
                if let Some(signed_transaction) = approval
                    .get("signed_result")
                    .and_then(|result| result.get("signed_transaction"))
                    .and_then(|value| value.as_str())
                {
                    data["signed_transaction"] = json!(signed_transaction);
                }
                Ok(json!({
                    "status": "ok",
                    "data": data
                }))
            }
            Some("record_transaction_hash") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let transaction_hash = required_test_str(request, "transaction_hash")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval.get("status").and_then(|value| value.as_str()) != Some("completed") {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet transaction hash can only be recorded after completion"
                    }));
                }
                if approval.get("intent").and_then(|value| value.as_str())
                    != Some("transaction_intent")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request is not a transaction"
                    }));
                }
                let mut signed_result =
                    approval.get("signed_result").cloned().unwrap_or_else(|| json!({}));
                signed_result["transaction_hash"] = json!(transaction_hash);
                signed_result["broadcast_recorded_at"] = json!(crate::auth::now_ts());
                approval["signed_result"] = signed_result;
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_request": approval.clone() }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock wallet op"
            })),
        }
    }
}
