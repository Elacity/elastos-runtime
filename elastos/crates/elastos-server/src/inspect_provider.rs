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
//! Data is read through the [`InspectSource`] trait. The provider holds a
//! strong `Arc<dyn InspectSource>`; each concrete source holds only a `Weak`
//! reference to the heavy runtime object it reads from, so registering the
//! provider on the registry never creates a reference cycle.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use elastos_common::CapsuleManifest;
use elastos_runtime::inspect::InspectScope;
use elastos_runtime::provider::{Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse};
use serde_json::{json, Value};

use crate::runtime::Runtime;

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
/// server's (fragmented) capsule tracking, and lets sources be aggregated.
#[async_trait]
pub trait InspectSource: Send + Sync {
    async fn inspect_list(&self) -> Vec<InspectEntry>;
    async fn inspect_get(&self, id: &str) -> Option<InspectEntry>;
}

// ── Sources ─────────────────────────────────────────────────────────

/// Source backed by the server `Runtime`'s running-capsule registry (the
/// capsules launched with a retained manifest — e.g. the single-VM serve path).
pub struct RuntimeInspectSource {
    runtime: Weak<Runtime>,
}

impl RuntimeInspectSource {
    pub fn new(runtime: Weak<Runtime>) -> Self {
        Self { runtime }
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

#[async_trait]
impl InspectSource for RuntimeInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        match self.runtime.upgrade() {
            Some(rt) => rt.list_capsules().await.into_iter().map(running_to_entry).collect(),
            None => Vec::new(),
        }
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        self.runtime.upgrade()?.get_capsule(id).await.map(running_to_entry)
    }
}

/// Source backed by the `ProviderRegistry`: the registered provider schemes
/// (the running provider capsules/services). Always populated on the main
/// product path. Thin — the registry does not carry per-provider manifests, so
/// these entries have no manifest (affordances/capabilities are empty until a
/// catalog/manifest source enriches them).
pub struct RegistryInspectSource {
    registry: Weak<ProviderRegistry>,
}

impl RegistryInspectSource {
    pub fn new(registry: Weak<ProviderRegistry>) -> Self {
        Self { registry }
    }

    fn scheme_entry(scheme: String) -> InspectEntry {
        InspectEntry {
            id: format!("provider:{scheme}"),
            name: scheme,
            status: "running".to_string(),
            capsule_type: "provider".to_string(),
            manifest: None,
        }
    }
}

#[async_trait]
impl InspectSource for RegistryInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        match self.registry.upgrade() {
            Some(reg) => {
                let mut schemes = reg.schemes().await;
                schemes.sort();
                schemes.into_iter().map(Self::scheme_entry).collect()
            }
            None => Vec::new(),
        }
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let scheme = id.strip_prefix("provider:")?;
        let reg = self.registry.upgrade()?;
        if reg.has_provider(scheme).await {
            Some(Self::scheme_entry(scheme.to_string()))
        } else {
            None
        }
    }
}

/// Source backed by the installed-capsule catalog on disk:
/// `<data_dir>/capsules/<name>/capsule.json`. Reads each capsule's full
/// manifest (rich detail: capabilities, affordances, provenance) and marks it
/// `running` when the scheme it `provides` is registered in the live registry,
/// else `installed`. This is the rich, manifest-backed source for the product.
pub struct CatalogInspectSource {
    capsules_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl CatalogInspectSource {
    pub fn new(capsules_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self { capsules_dir, registry }
    }

    /// The scheme a provider capsule serves, parsed from `provides`
    /// (e.g. `elastos://wallet/*` → `wallet`).
    fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
        manifest
            .provides
            .as_ref()?
            .strip_prefix("elastos://")
            .and_then(|rest| rest.split('/').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    async fn running_schemes(&self) -> std::collections::HashSet<String> {
        match self.registry.upgrade() {
            Some(reg) => reg.schemes().await.into_iter().collect(),
            None => std::collections::HashSet::new(),
        }
    }

    async fn read_entry(
        &self,
        name: &str,
        running: &std::collections::HashSet<String>,
    ) -> Option<InspectEntry> {
        let path = self.capsules_dir.join(name).join("capsule.json");
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        let manifest: CapsuleManifest = serde_json::from_str(&data).ok()?;
        let is_running = Self::provided_scheme(&manifest)
            .map(|s| running.contains(&s))
            .unwrap_or(false);
        Some(InspectEntry {
            id: format!("capsule:{name}"),
            name: manifest.name.clone(),
            status: if is_running { "running" } else { "installed" }.to_string(),
            capsule_type: format!("{:?}", manifest.capsule_type).to_lowercase(),
            manifest: Some(manifest),
        })
    }
}

#[async_trait]
impl InspectSource for CatalogInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let running = self.running_schemes().await;
        let mut out = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&self.capsules_dir).await {
            while let Ok(Some(dir_entry)) = rd.next_entry().await {
                let is_dir = dir_entry
                    .file_type()
                    .await
                    .map(|t| t.is_dir())
                    .unwrap_or(false);
                if !is_dir {
                    continue;
                }
                if let Some(name) = dir_entry.file_name().to_str() {
                    if let Some(entry) = self.read_entry(name, &running).await {
                        out.push(entry);
                    }
                }
            }
        }
        out
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let name = id.strip_prefix("capsule:")?;
        // Reject path traversal in the id.
        if name.contains('/') || name.contains("..") {
            return None;
        }
        let running = self.running_schemes().await;
        self.read_entry(name, &running).await
    }
}

/// Aggregates several sources into one. This is the unification point: the main
/// product path composes the runtime, catalog, and/or registry sources so the
/// browser Inspector shows every capsule any source knows about. De-duplicates
/// by id (first source wins).
pub struct AggregateInspectSource {
    sources: Vec<Arc<dyn InspectSource>>,
}

impl AggregateInspectSource {
    pub fn new(sources: Vec<Arc<dyn InspectSource>>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl InspectSource for AggregateInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for source in &self.sources {
            for entry in source.inspect_list().await {
                if seen.insert(entry.id.clone()) {
                    out.push(entry);
                }
            }
        }
        out
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        for source in &self.sources {
            if let Some(entry) = source.inspect_get(id).await {
                return Some(entry);
            }
        }
        None
    }
}

// ── Provider ────────────────────────────────────────────────────────

pub struct InspectProvider {
    source: Arc<dyn InspectSource>,
}

impl InspectProvider {
    pub fn new(source: Arc<dyn InspectSource>) -> Self {
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
        match request.get("op").and_then(Value::as_str).unwrap_or("") {
            // System-scope list. Upstream (gateway allow-list / capability
            // contract) gates who may reach this op.
            "capsules" => {
                let capsules: Vec<Value> = self
                    .source
                    .inspect_list()
                    .await
                    .iter()
                    .map(|e| {
                        json!({
                            "id": e.id,
                            "name": e.name,
                            "role": e.manifest.as_ref().and_then(|m| serde_json::to_value(&m.role).ok()),
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
                Some(id) => match self.source.inspect_get(id).await {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn probe_entry() -> InspectEntry {
        InspectEntry {
            id: "cap_probe_1".to_string(),
            name: "probe".to_string(),
            status: "running".to_string(),
            capsule_type: "wasm".to_string(),
            manifest: Some(probe_manifest()),
        }
    }

    fn provider_with_probe() -> InspectProvider {
        InspectProvider::new(Arc::new(MockSource { entries: vec![probe_entry()] }))
    }

    #[tokio::test]
    async fn capsules_lists_with_system_scope() {
        let resp = provider_with_probe()
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
        let resp = provider_with_probe()
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
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "nope" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "not_found");
    }

    #[tokio::test]
    async fn unknown_op_is_rejected() {
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "revoke" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "unknown_op");
    }

    #[tokio::test]
    async fn registry_source_lists_registered_schemes() {
        // A real registry with a registered provider scheme appears in inspect.
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(MockSchemeProvider)).await;
        let source = RegistryInspectSource::new(Arc::downgrade(&registry));

        let entries = source.inspect_list().await;
        assert!(entries.iter().any(|e| e.name == "wallet" && e.id == "provider:wallet"));
        assert!(source.inspect_get("provider:wallet").await.is_some());
        assert!(source.inspect_get("provider:nope").await.is_none());
    }

    #[tokio::test]
    async fn aggregate_source_unions_and_dedups() {
        let a: Arc<dyn InspectSource> = Arc::new(MockSource { entries: vec![probe_entry()] });
        let b: Arc<dyn InspectSource> = Arc::new(MockSource {
            entries: vec![
                probe_entry(), // duplicate id — should be deduped
                InspectEntry {
                    id: "provider:wallet".to_string(),
                    name: "wallet".to_string(),
                    status: "running".to_string(),
                    capsule_type: "provider".to_string(),
                    manifest: None,
                },
            ],
        });
        let agg = AggregateInspectSource::new(vec![a, b]);
        let entries = agg.inspect_list().await;
        assert_eq!(entries.len(), 2, "duplicate ids must be deduped");
        assert!(agg.inspect_get("provider:wallet").await.is_some());
    }

    fn write_capsule(capsules_dir: &std::path::Path, dir_name: &str, manifest: &Value) {
        let dir = capsules_dir.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(manifest).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn catalog_reads_installed_manifest_richly_without_leaking() {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        write_capsule(
            &capsules_dir,
            "probe",
            &serde_json::to_value(probe_manifest()).unwrap(),
        );

        let registry = Arc::new(ProviderRegistry::new());
        let source = CatalogInspectSource::new(capsules_dir, Arc::downgrade(&registry));

        let entries = source.inspect_list().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "capsule:probe");
        assert_eq!(entries[0].status, "installed"); // nothing registered

        // Detail is rich and never leaks the raw signature.
        let provider = InspectProvider::new(Arc::new(source));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "capsule:probe" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["affordances"][0]["id"], "ping");
        assert_eq!(resp["data"]["required_capabilities"][0], "elastos://storage/probe");
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("SECRET_SIGNATURE_MUST_NOT_LEAK"));
    }

    #[tokio::test]
    async fn catalog_marks_running_when_provided_scheme_registered() {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        write_capsule(
            &capsules_dir,
            "wallet-provider",
            &json!({
                "schema": "elastos.capsule/v1",
                "version": "0.1.0",
                "name": "wallet-provider",
                "role": "app",
                "type": "wasm",
                "entrypoint": "wallet.wasm",
                "provides": "elastos://wallet/*"
            }),
        );

        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(MockSchemeProvider)).await; // registers scheme "wallet"
        let source = CatalogInspectSource::new(capsules_dir, Arc::downgrade(&registry));

        let entry = source.inspect_get("capsule:wallet-provider").await.unwrap();
        assert_eq!(entry.status, "running");

        // Path-traversal ids are rejected.
        assert!(source.inspect_get("capsule:../etc").await.is_none());
    }

    // Minimal provider used only to register a "wallet" scheme in the registry.
    struct MockSchemeProvider;

    #[async_trait]
    impl Provider for MockSchemeProvider {
        async fn handle(&self, _r: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("unused".into()))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["wallet"]
        }
        fn name(&self) -> &'static str {
            "mock-wallet"
        }
    }
}
