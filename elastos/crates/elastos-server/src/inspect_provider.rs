//! Capsule Inspector provider (`elastos://inspect/*`).
//!
//! This is the product-side home of the Capsule Inspector. Both product
//! transports — the browser gateway and the capsule `carrier_invoke` bridge —
//! dispatch resource calls through `ProviderRegistry::send_raw(scheme, …)`, so
//! a single provider registered for the `inspect` scheme serves both (the
//! "one canonical path", Principle #10).
//!
//! The authority *decision* is the shared, transport-agnostic core in
//! `elastos_runtime::inspect` (`InspectScope`). Access is gated upstream — the
//! gateway app-allow-list for the browser, the capability-resource contract for
//! capsules — exactly as every other provider scheme is gated; this provider is
//! a read-only projection over runtime-owned state.
//!
//! Security (Principle #16): the projection allow-lists safe fields and never
//! echoes a bearer token, a raw signature, or any mutation handle. The raw
//! manifest `signature` is reduced to `signature_present`.
//!
//! Data is read through the [`InspectSource`] trait so the provider is
//! decoupled from where the server tracks capsules; `runtime::Runtime`
//! implements it.

use std::sync::Weak;

use async_trait::async_trait;
use elastos_common::CapsuleManifest;
use elastos_runtime::inspect::InspectScope;
use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
use serde_json::{json, Value};

/// One inspectable capsule, as seen by the provider.
#[derive(Debug, Clone)]
pub struct InspectEntry {
    pub id: String,
    pub name: String,
    pub status: String,
    pub capsule_type: String,
    /// Manifest the capsule was launched with, when retained.
    pub manifest: Option<CapsuleManifest>,
}

/// Read-only source of inspectable capsules. Decouples the provider from the
/// server's (currently fragmented) capsule tracking.
#[async_trait]
pub trait InspectSource: Send + Sync {
    async fn inspect_list(&self) -> Vec<InspectEntry>;
    async fn inspect_get(&self, id: &str) -> Option<InspectEntry>;
}

pub struct InspectProvider {
    source: Weak<dyn InspectSource>,
}

impl InspectProvider {
    pub fn new(source: Weak<dyn InspectSource>) -> Self {
        Self { source }
    }

    fn scope_label(scope: InspectScope) -> &'static str {
        match scope {
            InspectScope::System => "system",
            InspectScope::SelfOnly => "self",
        }
    }

    /// Project a capsule into the inspector wire contract (see
    /// docs/CAPSULE_INSPECTOR.md). Read-only; unknown fields are null rather
    /// than fabricated, and no bearer token / raw signature is ever included.
    fn project(entry: &InspectEntry) -> Value {
        fn field(v: &Value, key: &str) -> Value {
            v.get(key).cloned().unwrap_or(Value::Null)
        }

        // Serialize the manifest once and pick allow-listed fields. The raw
        // `signature` is deliberately *not* surfaced — only its presence.
        let manifest = entry
            .manifest
            .as_ref()
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or_else(|| json!({}));

        let signature_present = manifest
            .get("signature")
            .map(|s| s.is_string())
            .unwrap_or(false);

        // Affordances: flatten declared interface methods.
        let mut affordances = Vec::new();
        if let Some(interfaces) = manifest.get("interfaces").and_then(|v| v.as_array()) {
            for iface in interfaces {
                let iface_id = field(iface, "id");
                if let Some(methods) = iface.get("methods").and_then(|v| v.as_array()) {
                    for m in methods {
                        affordances.push(json!({
                            "interface": iface_id,
                            "id": field(m, "id"),
                            "risk": field(m, "risk"),
                            "approval": field(m, "approval"),
                            "audit": field(m, "audit"),
                            "description": field(m, "description"),
                        }));
                    }
                }
            }
        }

        json!({
            "id": entry.id,
            "name": entry.name,
            "version": field(&manifest, "version"),
            "role": field(&manifest, "role"),
            "type": field(&manifest, "type"),
            "description": field(&manifest, "description"),
            "author": field(&manifest, "author"),
            "identity": {
                "did": Value::Null,
                "cid": Value::Null,
                "trust_level": Value::Null,
                "signature_present": signature_present,
                "signed_by": Value::Null,
            },
            "manifest": {
                "schema": field(&manifest, "schema"),
                "entrypoint": field(&manifest, "entrypoint"),
            },
            "affordances": affordances,
            "required_capabilities": field(&manifest, "capabilities"),
            // Bearer-token object-capabilities have no central per-capsule grant
            // registry; observed grants come from the audit plane (not yet wired
            // into this provider). Empty rather than fabricated.
            "granted_capabilities": Value::Array(vec![]),
            "storage_namespaces": manifest
                .pointer("/permissions/storage")
                .cloned()
                .unwrap_or(Value::Null),
            "carrier": {
                "enabled": manifest.pointer("/permissions/carrier").cloned().unwrap_or(Value::Null),
                "endpoints": [],
                "peers": 0,
            },
            "provenance": {
                "signed_by": Value::Null,
                "version": field(&manifest, "version"),
                "installed_at": Value::Null,
                "cid": Value::Null,
                "signature_present": signature_present,
            },
            "audit": { "counts": { "total": 0, "denied": 0 }, "recent": [] },
            "processes": [{ "kind": entry.capsule_type, "status": entry.status }],
        })
    }

    async fn handle_op(&self, request: &Value) -> Value {
        let source = match self.source.upgrade() {
            Some(s) => s,
            None => return provider_error("unavailable", "inspect source is gone"),
        };

        match request.get("op").and_then(Value::as_str).unwrap_or("") {
            // System-scope list. Upstream (gateway allow-list / capability
            // contract) gates who may reach this op.
            "capsules" => {
                let entries = source.inspect_list().await;
                let capsules: Vec<Value> = entries
                    .iter()
                    .map(|e| {
                        json!({
                            "id": e.id,
                            "name": e.name,
                            "role": e.manifest.as_ref().map(|m| serde_json::to_value(&m.role).ok()),
                            "type": e.capsule_type,
                            "state": e.status,
                        })
                    })
                    .collect();
                json!({
                    "status": "ok",
                    "data": { "scope": Self::scope_label(InspectScope::System), "capsules": capsules },
                })
            }
            // System-scope detail.
            "capsule" => match request.get("id").and_then(Value::as_str) {
                Some(id) => match source.inspect_get(id).await {
                    Some(entry) => json!({ "status": "ok", "data": Self::project(&entry) }),
                    None => provider_error("not_found", "no such capsule"),
                },
                None => provider_error("invalid_request", "inspect/capsule requires an \"id\""),
            },
            other => provider_error("unknown_op", &format!("unknown inspect op: {other}")),
        }
    }
}

#[async_trait]
impl Provider for InspectProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "inspect provider uses raw operations; route via send_raw".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["inspect"]
    }

    fn name(&self) -> &'static str {
        "inspect-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        Ok(self.handle_op(request).await)
    }
}

fn provider_error(code: &str, message: &str) -> Value {
    json!({ "status": "error", "code": code, "message": message })
}

/// The server's running-capsule registry is one inspect source. (Browser-hosted
/// apps and registered provider schemes are additional sources to aggregate as
/// the server's capsule tracking is unified.)
#[async_trait]
impl InspectSource for crate::runtime::Runtime {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        self.list_capsules()
            .await
            .into_iter()
            .map(running_to_entry)
            .collect()
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        self.get_capsule(id).await.map(running_to_entry)
    }
}

fn running_to_entry(info: crate::runtime::RunningCapsuleInfo) -> InspectEntry {
    InspectEntry {
        id: info.id,
        name: info.name,
        status: info.status,
        capsule_type: format!("{:?}", info.capsule_type).to_lowercase(),
        manifest: Some(*info.manifest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn probe_manifest() -> CapsuleManifest {
        serde_json::from_value(json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "probe",
            "role": "app",
            "type": "wasm",
            "entrypoint": "probe.wasm",
            "capabilities": ["elastos://storage/probe"],
            "interfaces": [{
                "id": "elastos.probe/v1",
                "version": "1",
                "methods": [{ "id": "ping", "risk": "read", "approval": "none", "audit": "summary" }]
            }],
            "permissions": { "storage": ["localhost://WebSpaces/probe/"] },
            "signature": "SECRET_SIGNATURE_MUST_NOT_LEAK"
        }))
        .expect("probe manifest deserializes")
    }

    struct MockSource {
        entries: Vec<InspectEntry>,
    }

    #[async_trait]
    impl InspectSource for MockSource {
        async fn inspect_list(&self) -> Vec<InspectEntry> {
            self.entries.clone()
        }
        async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
            self.entries.iter().find(|e| e.id == id).cloned()
        }
    }

    fn provider_with_probe() -> (InspectProvider, Arc<dyn InspectSource>) {
        let source: Arc<dyn InspectSource> = Arc::new(MockSource {
            entries: vec![InspectEntry {
                id: "cap_probe_1".to_string(),
                name: "probe".to_string(),
                status: "running".to_string(),
                capsule_type: "wasm".to_string(),
                manifest: Some(probe_manifest()),
            }],
        });
        // Keep the Arc alive in the caller; provider holds a Weak.
        (InspectProvider::new(Arc::downgrade(&source)), source)
    }

    #[tokio::test]
    async fn capsules_lists_with_system_scope() {
        let (provider, _src) = provider_with_probe();
        let resp = provider
            .send_raw(&json!({ "op": "capsules" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["scope"], "system");
        assert_eq!(resp["data"]["capsules"][0]["id"], "cap_probe_1");
        assert_eq!(resp["data"]["capsules"][0]["name"], "probe");
    }

    #[tokio::test]
    async fn capsule_detail_renders_contract_without_leaking_authority() {
        let (provider, _src) = provider_with_probe();
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        let data = &resp["data"];
        assert_eq!(data["affordances"][0]["id"], "ping");
        assert_eq!(data["affordances"][0]["risk"], "read");
        assert_eq!(data["required_capabilities"][0], "elastos://storage/probe");
        assert_eq!(data["storage_namespaces"][0], "localhost://WebSpaces/probe/");
        assert_eq!(data["identity"]["signature_present"], true);

        // Principle #16: never echo the raw signature or any bearer token.
        let serialized = serde_json::to_string(data).unwrap();
        assert!(
            !serialized.contains("SECRET_SIGNATURE_MUST_NOT_LEAK"),
            "raw signature leaked into inspect output"
        );
        assert!(
            !serialized.contains("\"token\""),
            "bearer token field leaked into inspect output"
        );
    }

    #[tokio::test]
    async fn capsule_detail_unknown_id_is_not_found() {
        let (provider, _src) = provider_with_probe();
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "nope" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "not_found");
    }

    #[tokio::test]
    async fn unknown_op_is_rejected() {
        let (provider, _src) = provider_with_probe();
        let resp = provider.send_raw(&json!({ "op": "revoke" })).await.unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "unknown_op");
    }
}
