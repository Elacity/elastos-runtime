//! Runtime-owned Capsule Inspector provider (`elastos://inspect/*`).
//!
//! This is the live-object mirror: read-only, capability-gated by the existing
//! provider proxy/resource bridge, and projected through an allow-list so it does
//! not leak bearer tokens, raw signatures, host paths, or mutation handles.

mod dispatch;
mod planning;
mod projection;
mod sources;

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use serde_json::{json, Value};

pub use sources::{
    AggregateInspectSource, CatalogInspectSource, InspectEntry, InspectSource,
    RegistryInspectSource,
};

pub struct InspectProvider {
    source: Arc<dyn InspectSource>,
    registry: Weak<ProviderRegistry>,
}

impl InspectProvider {
    pub fn new(source: Arc<dyn InspectSource>) -> Self {
        Self {
            source,
            registry: Weak::new(),
        }
    }

    pub fn with_registry(source: Arc<dyn InspectSource>, registry: Weak<ProviderRegistry>) -> Self {
        Self { source, registry }
    }

    async fn handle_op(&self, request: &Value) -> Value {
        match request
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "capsules" => {
                let mut entries = self.source.inspect_list().await;
                entries.sort_by(|a, b| a.id.cmp(&b.id));
                ok(json!({
                    "schema": "elastos.inspect.capsules/v1",
                    "capsules": entries.iter().map(|entry| json!({
                        "id": entry.id,
                        "name": entry.name,
                        "kind": if entry.id.starts_with("provider:") { "provider" } else { "capsule" },
                        "state": entry.status,
                        "type": entry.capsule_type,
                    })).collect::<Vec<_>>()
                }))
            }
            "capsule" | "self" => {
                let id = request
                    .get("id")
                    .or_else(|| request.get("capsule_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match self.source.inspect_get(id).await {
                    Some(entry) => ok(projection::project(&entry)),
                    None => error("not_found", "inspect target not found"),
                }
            }
            "plan" => planning::plan(&self.source, request).await,
            "dispatch_approved" => {
                dispatch::dispatch_approved(&self.source, &self.registry, request).await
            }
            "revoke" => error("unsupported_operation", "inspect revoke is not implemented"),
            _ => error(
                "unsupported_operation",
                "unsupported inspect provider operation",
            ),
        }
    }
}

fn ok(data: Value) -> Value {
    json!({ "status": "ok", "data": data })
}

fn error(code: &str, message: &str) -> Value {
    json!({ "status": "error", "code": code, "message": message })
}

#[async_trait]
impl Provider for InspectProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "inspect provider supports raw runtime calls".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["inspect"]
    }

    fn name(&self) -> &'static str {
        "inspect"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        Ok(self.handle_op(request).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn provider_for_manifest(manifest: Value) -> (tempfile::TempDir, InspectProvider) {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        let capsule_dir = capsules_dir.join("exit-provider");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec(&json!({
                "external": {},
                "capsules": {
                    "exit-provider": {
                        "cid": "bafyexitprovider",
                        "sha256": "sha256-exit-provider",
                        "size": 42,
                        "platforms": []
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let source: Arc<dyn InspectSource> = Arc::new(CatalogInspectSource::new(
            capsules_dir,
            Arc::downgrade(&registry),
        ));
        (tmp, InspectProvider::new(source))
    }

    fn exit_manifest() -> Value {
        json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "exit-provider",
            "role": "provider",
            "type": "wasm",
            "entrypoint": "exit-provider.wasm",
            "author": "did:elastos:author",
            "provides": "elastos://exit/*",
            "authority": {
                "reason": "Runtime-owned exit provider",
                "capabilities": [
                    { "resource": "elastos://exit/*", "actions": ["execute"], "operations": ["open_stream"] },
                    { "resource": "elastos://exit/*", "actions": ["read"], "operations": ["status"] }
                ],
                "audit_events": ["exit.open_stream.requested", "exit.open_stream.denied"]
            },
            "capabilities": [],
            "signature": "do-not-echo"
        })
    }

    fn leaky_manifest() -> Value {
        let mut manifest = exit_manifest();
        manifest["entrypoint"] = json!("/host/private/exit-provider.wasm");
        manifest["capabilities"] = json!([
            "Bearer super-secret-token",
            "localhost://UsersAI/Documents/*"
        ]);
        manifest["permissions"] = json!({
            "storage": [
                "/host/private/storage",
                "localhost://UsersAI/Documents/*"
            ],
            "host_process": true
        });
        manifest["interfaces"] = json!([{
            "id": "elastos.exit.leaky",
            "version": "1",
            "methods": [{
                "id": "leaky",
                "risk": "read",
                "approval": "none",
                "audit": "none",
                "resource": "/host/private/socket",
                "operation": "dispatch_approved",
                "input_schema": {
                    "_runtime_invocation": { "source": "fake" },
                    "_Runtime_transfer": { "source": "fake" },
                    "authorization": "Bearer nested-secret",
                    "carrier_route": "private-route",
                    "connect_ticket": "ticket:nested-secret",
                    "raw_host_path": "/host/private/file",
                    "raw_signature": "raw-signature-secret",
                    "signature_raw": "signature-raw-secret",
                    "manifest_signature": "manifest-signature-secret"
                },
                "output_schema": {
                    "mutation_handle": "/api/provider/inspect/revoke",
                    "control_socket_path": "/tmp/elastos-inspect.sock",
                    "relay_ipc": "/tmp/elastos-relay.sock",
                    "adapter_ipc": "/tmp/elastos-adapter.sock"
                }
            }]
        }]);
        manifest["authority"]["capabilities"][0]["operations"] =
            json!(["status", "dispatch_approved", "revoke"]);
        manifest
    }

    #[tokio::test]
    async fn plan_reflects_provider_authority_without_dispatch() {
        let (_tmp, provider) = provider_for_manifest(exit_manifest()).await;
        let response = provider
            .send_raw(&json!({
                "op": "plan",
                "id": "capsule:exit-provider",
                "operation": "open_stream"
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        let data = &response["data"];
        assert_eq!(data["dispatch"], false);
        assert_eq!(data["mode"], "provider_authority");
        assert_eq!(
            data["execution"]["schema"],
            "elastos.inspect.execution-policy/v1"
        );
        assert_eq!(data["execution"]["mode"], "preview_only");
        assert_eq!(data["execution"]["can_dispatch"], false);
        assert_eq!(data["execution"]["can_mutate"], false);
        assert!(data["execution"]["approval_surface"].is_null());
        assert_eq!(data["capabilities"][0]["resource"], "elastos://exit/*");
        assert_eq!(data["capabilities"][0]["actions"][0], "execute");
        assert_eq!(data["audit_events"][0], "exit.open_stream.requested");
    }

    #[tokio::test]
    async fn projection_redacts_raw_signature_but_keeps_fingerprint() {
        let (_tmp, provider) = provider_for_manifest(exit_manifest()).await;
        let response = provider
            .send_raw(&json!({
                "op": "capsule",
                "id": "capsule:exit-provider"
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        let text = response.to_string();
        assert!(!text.contains("do-not-echo"));
        assert_eq!(
            response["data"]["provenance"]["author"],
            "did:elastos:author"
        );
        assert_eq!(response["data"]["provenance"]["cid"], "bafyexitprovider");
        assert_eq!(response["data"]["provenance"]["signature_present"], true);
        assert!(response["data"]["provenance"]["signature_fingerprint"].is_string());
        assert!(response["data"]["provenance"]["signed_by"].is_null());
    }

    #[tokio::test]
    async fn projection_reports_honest_authority_evidence_and_null_unavailable_facts() {
        let (_tmp, provider) = provider_for_manifest(exit_manifest()).await;
        let response = provider
            .send_raw(&json!({
                "op": "capsule",
                "id": "capsule:exit-provider"
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        let data = &response["data"];
        assert_eq!(
            data["provider_authority"]["reason"],
            "Runtime-owned exit provider"
        );
        assert_eq!(
            data["provider_authority"]["capabilities"][0]["resource"],
            "elastos://exit/*"
        );
        assert_eq!(data["authority"], data["provider_authority"]);
        assert_eq!(
            data["trust_evidence"]["schema"],
            "elastos.inspect.trust-evidence/v1"
        );
        assert_eq!(data["trust_evidence"]["cid_state"], "cid-published");
        assert_eq!(
            data["trust_evidence"]["manifest_signature"]["state"],
            "declared"
        );
        assert_eq!(
            data["trust_evidence"]["manifest_signature"]["fingerprint"],
            data["provenance"]["signature_fingerprint"]
        );
        assert_eq!(data["trust_evidence"]["verified"], false);
        assert!(data["trust_evidence"]["verified_by"].is_null());
        assert!(data["granted_capabilities"].is_null());
        assert!(data["audit"].is_null());
        assert!(data["spend_budget"].is_null());
        assert!(data["intent_proof"].is_null());
        assert!(data["audit_chain_attestation"].is_null());
    }

    #[tokio::test]
    async fn projection_redacts_secret_paths_routes_and_mutation_handles() {
        let (_tmp, provider) = provider_for_manifest(leaky_manifest()).await;
        let response = provider
            .send_raw(&json!({
                "op": "capsule",
                "id": "capsule:exit-provider"
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        let data = &response["data"];
        let text = data.to_string();
        for forbidden in [
            "/host/private",
            "Bearer super-secret-token",
            "Bearer nested-secret",
            "ticket:nested-secret",
            "connect_ticket",
            "carrier_route",
            "_runtime_invocation",
            "_Runtime_transfer",
            "raw_host_path",
            "raw_signature",
            "signature_raw",
            "raw-signature-secret",
            "signature-raw-secret",
            "manifest-signature-secret",
            "mutation_handle",
            "control_socket_path",
            "relay_ipc",
            "adapter_ipc",
            "/tmp/elastos-inspect.sock",
            "/tmp/elastos-relay.sock",
            "/tmp/elastos-adapter.sock",
            "dispatch_approved",
            "/api/provider/inspect/revoke",
        ] {
            assert!(!text.contains(forbidden), "{forbidden} leaked in {text}");
        }
        assert!(data["manifest"]["entrypoint"].is_null());
        assert!(data["affordances"][0]["methods"][0]["resource"].is_null());
        assert!(data["affordances"][0]["methods"][0]["operation"].is_null());
        assert!(data["storage_namespaces"][0].is_null());
        assert_eq!(
            data["storage_namespaces"][1],
            "localhost://UsersAI/Documents/*"
        );
        assert_eq!(
            data["required_capabilities"][1],
            "localhost://UsersAI/Documents/*"
        );
    }

    #[tokio::test]
    async fn plan_can_preview_canonical_provider_resource_contract() {
        let (_tmp, provider) = provider_for_manifest(exit_manifest()).await;
        let response = provider
            .send_raw(&json!({
                "op": "plan",
                "scheme": "inspect",
                "operation": "capsules",
                "request": {}
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["mode"], "provider_resource");
        assert_eq!(
            response["data"]["capabilities"][0]["resource"],
            "elastos://inspect/capsules"
        );
        assert_eq!(response["data"]["capabilities"][0]["actions"][0], "read");
        assert_eq!(response["data"]["execution"]["mode"], "preview_only");
        assert_eq!(response["data"]["execution"]["can_dispatch"], false);
    }

    #[tokio::test]
    async fn dispatch_rejects_capsule_supplied_runtime_metadata() {
        let (_tmp, provider) = provider_for_manifest(exit_manifest()).await;
        for field in [
            "_runtime_invocation",
            "_runtime_transfer",
            "_runtime_probe",
            "connect_ticket",
            "carrier_route",
            "carrier",
        ] {
            let response = provider
                .send_raw(&json!({
                    "op": "dispatch_approved",
                    "id": "capsule:exit-provider",
                    "operation": "status",
                    "request": {
                        field: true,
                    }
                }))
                .await
                .unwrap();
            assert_eq!(response["status"], "error");
            assert_eq!(response["code"], "invalid_request");
            assert!(
                response["message"].as_str().unwrap().contains(field),
                "{response}"
            );
        }
    }
}
