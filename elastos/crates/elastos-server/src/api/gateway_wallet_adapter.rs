use std::time::{SystemTime, UNIX_EPOCH};

use elastos_runtime::provider::{
    ProviderInvocation, ProviderInvocationTransport, ProviderRegistry, ProviderTransfer,
};
use elastos_wallet_contract::{
    WalletProviderOperationV2, WalletProviderRequestV2, WalletProviderResponseV2,
    MAX_INVOCATION_TTL_SECS, WALLET_BUS_OPERATION,
};
use rand::RngCore;

use super::gateway_home_token::RuntimeWalletAuthority;

const RUNTIME_PROVIDER_ID: &str = "runtime";
const WALLET_PROVIDER_ID: &str = "wallet";

/// Private Runtime adapter for authority-bound Wallet Bus v2 calls.
pub(in crate::api) struct RuntimeWalletAdapter<'a> {
    registry: &'a ProviderRegistry,
    authority: &'a RuntimeWalletAuthority,
}

impl<'a> RuntimeWalletAdapter<'a> {
    pub(in crate::api) fn new(
        registry: &'a ProviderRegistry,
        authority: &'a RuntimeWalletAuthority,
    ) -> Self {
        Self {
            registry,
            authority,
        }
    }

    pub(in crate::api) async fn invoke(
        &self,
        operation: WalletProviderOperationV2,
    ) -> anyhow::Result<WalletProviderResponseV2> {
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow::anyhow!("Runtime clock is before the Unix epoch"))?
            .as_secs();
        let mut nonce = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut nonce);
        self.invoke_with_runtime_fields(
            format!("wallet-request:{}", hex::encode(nonce)),
            issued_at,
            operation,
        )
        .await
    }

    async fn invoke_with_runtime_fields(
        &self,
        request_id: String,
        issued_at: u64,
        operation: WalletProviderOperationV2,
    ) -> anyhow::Result<WalletProviderResponseV2> {
        let request = WalletProviderRequestV2::new(
            self.authority.verified_context(),
            request_id,
            issued_at,
            issued_at.saturating_add(MAX_INVOCATION_TTL_SECS),
            operation,
        )
        .map_err(|err| anyhow::anyhow!("invalid Runtime Wallet request: {err}"))?;
        let response = self
            .registry
            .invoke_provider(ProviderInvocation {
                source: RUNTIME_PROVIDER_ID.to_string(),
                target: WALLET_PROVIDER_ID.to_string(),
                op: WALLET_BUS_OPERATION.to_string(),
                request: serde_json::json!({
                    "op": WALLET_BUS_OPERATION,
                    "request": request,
                }),
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .map_err(|err| anyhow::anyhow!("Wallet provider invocation failed: {err}"))?;
        let status = response
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Wallet provider response is missing status"))?;
        if status != "ok" {
            let message = response
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Wallet provider rejected the request");
            anyhow::bail!("Wallet provider response failed: {message}");
        }
        let data = response
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("Wallet provider response is missing v2 data"))?;
        let bytes = serde_json::to_vec(data)?;
        WalletProviderResponseV2::decode_for_request(&bytes, &request)
            .map_err(|err| anyhow::anyhow!("invalid Wallet provider v2 response: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::http::{HeaderMap, HeaderValue};
    use base64::Engine as _;
    use elastos_runtime::provider::{
        Provider, ProviderCarrierInvoker, ProviderCarrierRoute, ProviderError, ResourceRequest,
        ResourceResponse,
    };
    use elastos_wallet_contract::{
        VerifiedWalletInvocationContext, WalletProviderRequestV2, WalletResultV2,
        WALLET_REQUEST_SCHEMA,
    };
    use tokio::sync::Mutex;

    use super::*;
    use crate::api::gateway::gateway_home_token::{
        issue_home_launch_token_with_context, local_home_launch_token_context,
        require_runtime_wallet_authority,
    };

    const ISSUED_AT: u64 = 1_800_000_000;
    const REQUEST_ID: &str = "wallet-request:0123456789abcdef0123456789abcdef";

    #[derive(Clone, Copy)]
    enum ResponseMutation {
        None,
        TransportFailure,
        MalformedEnvelope,
        StructuredProofError,
        StaleProtocol,
        MixedSchema,
        Substitute(&'static str),
    }

    struct MockWalletProvider {
        requests: Mutex<Vec<serde_json::Value>>,
        mutation: ResponseMutation,
    }

    impl MockWalletProvider {
        fn new(mutation: ResponseMutation) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                mutation,
            }
        }
    }

    #[async_trait]
    impl Provider for MockWalletProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "test Wallet provider supports only wallet_contract".to_string(),
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
            self.requests.lock().await.push(request.clone());
            if matches!(self.mutation, ResponseMutation::TransportFailure) {
                return Err(ProviderError::Provider(
                    "simulated Wallet transport failure".to_string(),
                ));
            }
            let wallet_request: WalletProviderRequestV2 =
                serde_json::from_value(request.get("request").cloned().ok_or_else(|| {
                    ProviderError::Provider("missing Wallet request".to_string())
                })?)
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
            let mut response = serde_json::to_value(WalletProviderResponseV2::for_request(
                &wallet_request,
                match self.mutation {
                    ResponseMutation::StructuredProofError => WalletResultV2::Error {
                        code: "invalid_proof".to_string(),
                        message: "EOA proof was rejected".to_string(),
                    },
                    _ => WalletResultV2::Ok {
                        data: serde_json::json!({"accepted": true}),
                    },
                },
            ))
            .map_err(|err| ProviderError::Provider(err.to_string()))?;
            match self.mutation {
                ResponseMutation::None | ResponseMutation::StructuredProofError => {}
                ResponseMutation::TransportFailure => unreachable!(),
                ResponseMutation::MalformedEnvelope => {
                    return Ok(serde_json::json!({"status": "ok", "data": {}}));
                }
                ResponseMutation::StaleProtocol => {
                    response["protocol_version"] = serde_json::json!("1.0");
                }
                ResponseMutation::MixedSchema => {
                    response["schema"] = serde_json::json!(WALLET_REQUEST_SCHEMA);
                }
                ResponseMutation::Substitute("operation") => {
                    response["operation"] = serde_json::json!("challenge");
                }
                ResponseMutation::Substitute(field) => {
                    response[field] = serde_json::json!("substituted");
                }
            }
            Ok(serde_json::json!({"status": "ok", "data": response}))
        }
    }

    #[derive(Default)]
    struct CountingCarrierInvoker {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ProviderCarrierInvoker for CountingCarrierInvoker {
        async fn invoke_carrier_provider(
            &self,
            _route: &ProviderCarrierRoute,
            _invocation: &ProviderInvocation,
            _request: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Provider(
                "Wallet adapter must not use Carrier".to_string(),
            ))
        }
    }

    fn test_authority() -> RuntimeWalletAuthority {
        RuntimeWalletAuthority::from_verified_context(
            VerifiedWalletInvocationContext::new(
                "principal:did:key:test",
                "session:test",
                Some("proof:test".to_string()),
                "grant:test",
                "wallet-metamask",
                "launch:0123456789abcdef0123456789abcdef",
            )
            .unwrap(),
        )
    }

    async fn test_registry(
        mutation: ResponseMutation,
    ) -> (Arc<ProviderRegistry>, Arc<MockWalletProvider>) {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(MockWalletProvider::new(mutation));
        registry
            .register_sub_provider("wallet", provider.clone())
            .await
            .unwrap();
        (registry, provider)
    }

    fn list_accounts() -> WalletProviderOperationV2 {
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        }
    }

    #[test]
    fn wallet_v2_authority_projection_preserves_verified_actor_and_launch_id() {
        let data_dir = tempfile::tempdir().unwrap();
        let launch_context = local_home_launch_token_context(data_dir.path()).unwrap();
        let token = issue_home_launch_token_with_context(
            data_dir.path(),
            "wallet-metamask",
            &launch_context,
        )
        .unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&token)
                .unwrap(),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:61180"));
        headers.insert("origin", HeaderValue::from_static("null"));
        headers.insert(
            "x-elastos-home-token",
            HeaderValue::from_str(&token).unwrap(),
        );

        let authority =
            require_runtime_wallet_authority(data_dir.path(), &headers, &["wallet-metamask"])
                .unwrap();
        let verified = authority.verified_context();

        assert_eq!(verified.principal_id(), launch_context.principal_id);
        assert_eq!(verified.session_id(), launch_context.session_id);
        assert_eq!(
            verified.proof_binding_id(),
            launch_context.proof_binding_id.as_deref()
        );
        assert_eq!(verified.grant_id(), launch_context.grant_id);
        assert_eq!(verified.actor(), "wallet-metamask");
        assert_eq!(verified.launch_id(), envelope["payload"]["launch_id"]);
    }

    #[tokio::test]
    async fn wallet_v2_dispatch_is_runtime_local_and_wallet_contract_only() {
        let (registry, provider) = test_registry(ResponseMutation::None).await;
        let carrier = Arc::new(CountingCarrierInvoker::default());
        registry.set_carrier_invoker(carrier.clone()).await;
        let authority = test_authority();

        RuntimeWalletAdapter::new(&registry, &authority)
            .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, list_accounts())
            .await
            .unwrap();

        assert_eq!(carrier.calls.load(Ordering::SeqCst), 0);
        let requests = provider.requests.lock().await;
        let request = requests.first().unwrap();
        assert_eq!(request["op"], WALLET_BUS_OPERATION);
        assert_eq!(request["_runtime_invocation"]["source"], "runtime");
        assert_eq!(request["_runtime_invocation"]["target"], "wallet");
        assert_eq!(request["_runtime_invocation"]["op"], WALLET_BUS_OPERATION);
        assert_eq!(
            request["_runtime_invocation"]["capability"],
            "provider:runtime->wallet:wallet_contract"
        );
        assert_eq!(
            request["_runtime_invocation"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(
            request["_runtime_invocation"]["carrier"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn wallet_v2_caller_fields_cannot_override_runtime_bindings() {
        let (registry, provider) = test_registry(ResponseMutation::None).await;
        let authority = test_authority();
        let operation = WalletProviderOperationV2::RequestApproval {
            account_id: "account:test".to_string(),
            chain_namespace: "eip155".to_string(),
            intent: "transaction".to_string(),
            resource: "https://example.test/resource".to_string(),
            reason: "test approval".to_string(),
            payload: serde_json::json!({
                "authority": "caller-authority",
                "request_id": "caller-request",
                "lifecycle_id": "caller-lifecycle",
                "audit_id": "caller-audit",
                "_runtime_invocation": "caller-envelope",
            }),
            expires_at: ISSUED_AT + 600,
        };

        RuntimeWalletAdapter::new(&registry, &authority)
            .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, operation)
            .await
            .unwrap();

        let requests = provider.requests.lock().await;
        let request = &requests[0]["request"];
        assert_eq!(request["schema"], WALLET_REQUEST_SCHEMA);
        assert_eq!(request["request_id"], REQUEST_ID);
        assert_eq!(
            request["authority"]["principal_id"],
            "principal:did:key:test"
        );
        assert_eq!(request["authority"]["actor"], "wallet-metamask");
        assert_eq!(
            request["authority"]["capability"],
            "wallet:approval:request"
        );
        assert_eq!(request["authority"]["intent"], "wallet.approval.request");
        assert_ne!(request["lifecycle_id"], "caller-lifecycle");
        assert_ne!(request["audit_id"], "caller-audit");
        assert_eq!(
            request["operation"]["params"]["payload"]["authority"],
            "caller-authority"
        );
        assert!(request.get("_runtime_invocation").is_none());
    }

    #[tokio::test]
    async fn wallet_v2_rejects_stale_or_mixed_provider_responses() {
        for mutation in [
            ResponseMutation::StaleProtocol,
            ResponseMutation::MixedSchema,
        ] {
            let (registry, _) = test_registry(mutation).await;
            let authority = test_authority();
            let error = RuntimeWalletAdapter::new(&registry, &authority)
                .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, list_accounts())
                .await
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("invalid Wallet provider v2 response"));
        }
    }

    #[tokio::test]
    async fn wallet_v2_transport_and_malformed_responses_remain_adapter_errors() {
        for mutation in [
            ResponseMutation::TransportFailure,
            ResponseMutation::MalformedEnvelope,
        ] {
            let (registry, _) = test_registry(mutation).await;
            let authority = test_authority();
            RuntimeWalletAdapter::new(&registry, &authority)
                .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, list_accounts())
                .await
                .unwrap_err();
        }
    }

    #[tokio::test]
    async fn wallet_v2_preserves_structured_proof_errors() {
        let (registry, _) = test_registry(ResponseMutation::StructuredProofError).await;
        let authority = test_authority();
        let response = RuntimeWalletAdapter::new(&registry, &authority)
            .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, list_accounts())
            .await
            .unwrap();

        assert_eq!(
            response.result,
            WalletResultV2::Error {
                code: "invalid_proof".to_string(),
                message: "EOA proof was rejected".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn wallet_v2_rejects_every_substituted_response_binding() {
        let cases = [
            ("request_id", list_accounts()),
            ("operation", list_accounts()),
            ("audit_id", list_accounts()),
            ("lifecycle_id", list_accounts()),
            ("session_binding", list_accounts()),
            (
                "account_binding",
                WalletProviderOperationV2::RequestApproval {
                    account_id: "account:test".to_string(),
                    chain_namespace: "eip155".to_string(),
                    intent: "transaction".to_string(),
                    resource: "https://example.test/resource".to_string(),
                    reason: "test approval".to_string(),
                    payload: serde_json::json!({"value": "0x1"}),
                    expires_at: ISSUED_AT + 600,
                },
            ),
            (
                "approval_binding",
                WalletProviderOperationV2::ApproveAndSignManaged {
                    request_id: "approval:test".to_string(),
                    reason: "test approval".to_string(),
                },
            ),
        ];

        for (field, operation) in cases {
            let (registry, _) = test_registry(ResponseMutation::Substitute(field)).await;
            let authority = test_authority();
            let error = RuntimeWalletAdapter::new(&registry, &authority)
                .invoke_with_runtime_fields(REQUEST_ID.to_string(), ISSUED_AT, operation)
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("does not match its authority-bound request"),
                "field {field} was not rejected: {error}"
            );
        }
    }
}
