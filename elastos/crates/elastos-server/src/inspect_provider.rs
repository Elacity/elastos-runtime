//! Runtime-owned Capsule Inspector provider (`elastos://inspect/*`).
//!
//! This is the live-object mirror: read-only, capability-gated by the existing
//! provider proxy/Carrier bridge, and projected through an allow-list so it does
//! not leak bearer tokens, raw signatures, host paths, or mutation handles.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use elastos_common::CapsuleManifest;
use elastos_runtime::invoke;
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderInvocation, ProviderInvocationTransport, ProviderRegistry,
    ProviderTransfer, ResourceRequest, ResourceResponse,
};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::provider_resource::{build_capability_resource, provider_operation_action};

#[derive(Debug, Clone)]
pub struct InspectEntry {
    pub id: String,
    pub name: String,
    pub status: String,
    pub capsule_type: String,
    pub manifest: Option<CapsuleManifest>,
    pub cid: Option<String>,
}

#[async_trait]
pub trait InspectSource: Send + Sync {
    async fn inspect_list(&self) -> Vec<InspectEntry>;
    async fn inspect_get(&self, id: &str) -> Option<InspectEntry>;
}

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
            cid: None,
        }
    }
}

#[async_trait]
impl InspectSource for RegistryInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let Some(registry) = self.registry.upgrade() else {
            return Vec::new();
        };
        let mut schemes = registry.schemes().await;
        schemes.extend(registry.sub_provider_schemes().await);
        schemes.sort();
        schemes.dedup();
        schemes.into_iter().map(Self::scheme_entry).collect()
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let scheme = id.strip_prefix("provider:")?;
        let registry = self.registry.upgrade()?;
        let is_known = registry.has_provider(scheme).await
            || registry
                .sub_provider_schemes()
                .await
                .iter()
                .any(|known| known == scheme);
        is_known.then(|| Self::scheme_entry(scheme.to_string()))
    }
}

pub struct CatalogInspectSource {
    capsules_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl CatalogInspectSource {
    pub fn new(capsules_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self {
            capsules_dir,
            registry,
        }
    }

    fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
        manifest
            .provides
            .as_ref()?
            .strip_prefix("elastos://")
            .and_then(|rest| rest.split('/').next())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    async fn running_schemes(&self) -> std::collections::HashSet<String> {
        let Some(registry) = self.registry.upgrade() else {
            return std::collections::HashSet::new();
        };
        let mut schemes: std::collections::HashSet<String> =
            registry.schemes().await.into_iter().collect();
        schemes.extend(registry.sub_provider_schemes().await);
        schemes
    }

    async fn catalog_cids(&self) -> std::collections::HashMap<String, String> {
        let Some(path) = self
            .capsules_dir
            .parent()
            .map(|path| path.join("components.json"))
        else {
            return std::collections::HashMap::new();
        };
        let Ok(data) = tokio::fs::read_to_string(path).await else {
            return std::collections::HashMap::new();
        };
        serde_json::from_str::<crate::setup::ComponentsManifest>(&data)
            .map(|manifest| {
                manifest
                    .capsules
                    .into_iter()
                    .filter(|(_, entry)| !entry.cid.is_empty())
                    .map(|(name, entry)| (name, entry.cid))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn read_entry(
        &self,
        name: &str,
        running: &std::collections::HashSet<String>,
        cid: Option<String>,
    ) -> Option<InspectEntry> {
        if name.contains('/') || name.contains("..") {
            return None;
        }
        let path = self.capsules_dir.join(name).join("capsule.json");
        let data = tokio::fs::read_to_string(path).await.ok()?;
        let manifest: CapsuleManifest = serde_json::from_str(&data).ok()?;
        let is_running = Self::provided_scheme(&manifest)
            .map(|scheme| running.contains(&scheme))
            .unwrap_or(false);
        Some(InspectEntry {
            id: format!("capsule:{name}"),
            name: manifest.name.clone(),
            status: if is_running { "running" } else { "installed" }.to_string(),
            capsule_type: format!("{:?}", manifest.capsule_type).to_lowercase(),
            manifest: Some(manifest),
            cid,
        })
    }
}

#[async_trait]
impl InspectSource for CatalogInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let running = self.running_schemes().await;
        let cids = self.catalog_cids().await;
        let mut entries = Vec::new();
        let Ok(mut dirs) = tokio::fs::read_dir(&self.capsules_dir).await else {
            return entries;
        };
        while let Ok(Some(dir)) = dirs.next_entry().await {
            let is_dir = dir.file_type().await.map(|ty| ty.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let Some(name) = dir.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(entry) = self
                .read_entry(&name, &running, cids.get(&name).cloned())
                .await
            {
                entries.push(entry);
            }
        }
        entries
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let name = id.strip_prefix("capsule:").unwrap_or(id);
        let running = self.running_schemes().await;
        let cid = self.catalog_cids().await.get(name).cloned();
        self.read_entry(name, &running, cid).await
    }
}

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
        let mut entries = Vec::new();
        for source in &self.sources {
            for entry in source.inspect_list().await {
                if seen.insert(entry.id.clone()) {
                    entries.push(entry);
                }
            }
        }
        entries
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

    fn preview_execution_policy() -> Value {
        json!({
            "schema": "elastos.inspect.execution-policy/v1",
            "mode": "preview_only",
            "can_dispatch": false,
            "can_mutate": false,
            "approval_surface": Value::Null,
        })
    }

    fn approved_execution_policy() -> Value {
        json!({
            "schema": "elastos.inspect.execution-policy/v1",
            "mode": "approved_dispatch",
            "can_dispatch": true,
            "can_mutate": true,
            "approval_surface": "inbox",
        })
    }

    fn signature_fingerprint(signature: &str) -> String {
        hex::encode(Sha256::digest(signature.as_bytes()))
            .chars()
            .take(16)
            .collect()
    }

    fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
        manifest
            .provides
            .as_ref()?
            .strip_prefix("elastos://")
            .and_then(|rest| rest.split('/').next())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    fn project(entry: &InspectEntry) -> Value {
        let manifest = entry
            .manifest
            .as_ref()
            .and_then(|manifest| serde_json::to_value(manifest).ok())
            .unwrap_or_else(|| json!({}));
        let signature = manifest.get("signature").and_then(Value::as_str);
        let authority = manifest
            .get("authority")
            .map(|authority| {
                json!({
                    "reason": authority.get("reason").cloned().unwrap_or(Value::Null),
                    "capabilities": authority.get("capabilities").cloned().unwrap_or(Value::Null),
                    "audit_events": authority.get("audit_events").cloned().unwrap_or(Value::Null),
                })
            })
            .unwrap_or(Value::Null);
        json!({
            "schema": "elastos.inspect.object/v1",
            "kind": if entry.id.starts_with("provider:") { "provider" } else { "capsule" },
            "id": entry.id,
            "name": entry.name,
            "state": entry.status,
            "type": entry.capsule_type,
            "manifest": {
                "schema": manifest.get("schema").cloned().unwrap_or(Value::Null),
                "version": manifest.get("version").cloned().unwrap_or(Value::Null),
                "role": manifest.get("role").cloned().unwrap_or(Value::Null),
                "entrypoint": manifest.get("entrypoint").cloned().unwrap_or(Value::Null),
                "provides": manifest.get("provides").cloned().unwrap_or(Value::Null),
            },
            "affordances": manifest.get("interfaces").cloned().unwrap_or_else(|| json!([])),
            "required_capabilities": manifest.get("capabilities").cloned().unwrap_or_else(|| json!([])),
            "granted_capabilities": [],
            "storage_namespaces": manifest.pointer("/permissions/storage").cloned().unwrap_or(Value::Null),
            "carrier": {
                "enabled": manifest.pointer("/permissions/carrier").cloned().unwrap_or(Value::Null),
                "endpoints": [],
            },
            "authority": authority,
            "provenance": {
                "author": manifest.get("author").cloned().unwrap_or(Value::Null),
                "cid": entry.cid,
                "signature_present": signature.is_some(),
                "signature_fingerprint": signature.map(Self::signature_fingerprint),
                "signed_by": Value::Null,
            },
            "audit": {
                "counts": { "total": 0, "denied": 0, "attested": 0 },
                "recent": [],
            },
            "processes": [{ "kind": entry.capsule_type, "status": entry.status }],
        })
    }

    async fn plan(&self, request: &Value) -> Value {
        let operation = request
            .get("operation")
            .or_else(|| request.get("provider_operation"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if operation.is_empty() {
            return error("invalid_request", "inspect plan requires operation");
        }

        if let Some(scheme) = request.get("scheme").and_then(Value::as_str) {
            let body = request.get("request").cloned().unwrap_or_else(|| json!({}));
            return match build_capability_resource(scheme, operation, &body) {
                Ok(resource) => ok(json!({
                    "schema": "elastos.inspect.gate-preview/v1",
                    "mode": "provider_resource",
                    "provider": scheme,
                    "operation": operation,
                    "capabilities": [{
                        "resource": resource,
                        "actions": provider_operation_action(scheme, operation)
                            .map(|action| vec![action.to_string()])
                            .unwrap_or_default(),
                    }],
                    "execution": Self::preview_execution_policy(),
                    "dispatch": false,
                })),
                Err(message) => error("invalid_request", &message),
            };
        }

        let id = request
            .get("id")
            .or_else(|| request.get("capsule_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty() {
            return error("invalid_request", "inspect plan requires id or scheme");
        }
        let Some(entry) = self.source.inspect_get(id).await else {
            return error("not_found", "inspect target not found");
        };
        let Some(authority) = entry
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.authority.as_ref())
        else {
            return error(
                "no_authority",
                "inspect target has no provider authority metadata",
            );
        };
        match invoke::plan_provider_operation(authority, operation) {
            Ok(plan) => ok(json!({
                "schema": "elastos.inspect.gate-preview/v1",
                "mode": "provider_authority",
                "id": entry.id,
                "provider": entry.name,
                "operation": operation,
                "capabilities": plan.resources.iter().map(|resource| json!({
                    "resource": resource,
                    "actions": plan.actions.iter().map(ToString::to_string).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "audit_events": plan.audit_events,
                "execution": Self::preview_execution_policy(),
                "dispatch": false,
            })),
            Err(invoke::InvokeError::UnknownOperation(_)) => error(
                "unknown_operation",
                "operation is not declared by target authority",
            ),
            Err(invoke::InvokeError::UnknownDeclaredAction(action)) => error(
                "invalid_authority",
                &format!("unknown declared action: {action}"),
            ),
            Err(_) => error("invalid_authority", "invalid authority metadata"),
        }
    }

    async fn dispatch_approved(&self, request: &Value) -> Value {
        let id = request
            .get("id")
            .or_else(|| request.get("capsule_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty() {
            return error("invalid_request", "inspect dispatch requires id");
        }
        let operation = request
            .get("operation")
            .or_else(|| request.get("provider_operation"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if operation.is_empty() {
            return error("invalid_request", "inspect dispatch requires operation");
        }
        let Some(entry) = self.source.inspect_get(id).await else {
            return error("not_found", "inspect target not found");
        };
        let Some(manifest) = entry.manifest.as_ref() else {
            return error(
                "invalid_target",
                "inspect dispatch target is not manifest-backed",
            );
        };
        let Some(authority) = manifest.authority.as_ref() else {
            return error(
                "no_authority",
                "inspect target has no provider authority metadata",
            );
        };
        let Some(target) = Self::provided_scheme(manifest) else {
            return error(
                "invalid_target",
                "inspect dispatch target has no provider scheme",
            );
        };
        let plan = match invoke::plan_provider_operation(authority, operation) {
            Ok(plan) => plan,
            Err(invoke::InvokeError::UnknownOperation(_)) => {
                return error(
                    "unknown_operation",
                    "operation is not declared by target authority",
                )
            }
            Err(invoke::InvokeError::UnknownDeclaredAction(action)) => {
                return error(
                    "invalid_authority",
                    &format!("unknown declared action: {action}"),
                )
            }
            Err(_) => return error("invalid_authority", "invalid authority metadata"),
        };
        let mut provider_request = request.get("request").cloned().unwrap_or_else(|| json!({}));
        if let Some(field) = inspect_hidden_runtime_metadata_field(&provider_request) {
            return error(
                "invalid_request",
                &format!(
                    "inspect dispatch request must not predeclare Runtime metadata field {field}"
                ),
            );
        }
        let Some(provider_object) = provider_request.as_object_mut() else {
            return error(
                "invalid_request",
                "inspect dispatch request must be a JSON object",
            );
        };
        provider_object.insert("op".to_string(), Value::String(operation.to_string()));

        let Some(registry) = self.registry.upgrade() else {
            return error(
                "runtime_unavailable",
                "inspect dispatch registry unavailable",
            );
        };
        match registry
            .invoke_provider(ProviderInvocation {
                source: "inspect".to_string(),
                target: target.clone(),
                op: operation.to_string(),
                request: provider_request,
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
        {
            Ok(provider_response) => ok(json!({
                "schema": "elastos.inspect.dispatch-result/v1",
                "mode": "provider_authority",
                "id": entry.id,
                "provider": entry.name,
                "target": target,
                "operation": operation,
                "capabilities": plan.resources.iter().map(|resource| json!({
                    "resource": resource,
                    "actions": plan.actions.iter().map(ToString::to_string).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "audit_events": plan.audit_events,
                "execution": Self::approved_execution_policy(),
                "provider_response": provider_response,
            })),
            Err(err) => error("dispatch_failed", &err.to_string()),
        }
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
                    Some(entry) => ok(Self::project(&entry)),
                    None => error("not_found", "inspect target not found"),
                }
            }
            "plan" => self.plan(request).await,
            "dispatch_approved" => self.dispatch_approved(request).await,
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

fn inspect_hidden_runtime_metadata_field(request: &Value) -> Option<&str> {
    request
        .as_object()?
        .keys()
        .find(|key| {
            key.starts_with("_runtime")
                || matches!(key.as_str(), "connect_ticket" | "carrier_route" | "carrier")
        })
        .map(String::as_str)
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
        assert_eq!(response["data"]["provenance"]["signature_present"], true);
        assert!(response["data"]["provenance"]["signature_fingerprint"].is_string());
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
