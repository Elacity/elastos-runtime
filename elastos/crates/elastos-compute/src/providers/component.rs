//! WASM Component Model provider for the ElastOS component ABI.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use wasmtime::component::{bindgen, Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use elastos_common::{
    CapsuleId, CapsuleManifest, CapsuleRuntimeAbi, CapsuleStatus, CapsuleType, ElastosError, Result,
};
use elastos_guest::{
    bus as guest_bus,
    runtime::{ResponseEnvelope, RuntimeRequest, RuntimeResponse},
};

use crate::{CapsuleHandle, CapsuleInfo, ComputeProvider};

use super::BridgeHostcall;

bindgen!({
    path: "../../wit",
    world: "product-capsule-v1",
});

const COMPONENT_FUEL: u64 = 100_000_000;
const COMPONENT_MEMORY_SIZE: usize = 128 * 1024 * 1024;

struct ComponentActivation {
    engine: Engine,
    component: Component,
    status: CapsuleStatus,
    manifest: CapsuleManifest,
}

struct ComponentState {
    hostcall: Option<BridgeHostcall>,
    capsule_id: String,
    principal_id: Option<String>,
    limits: StoreLimits,
}

/// Component compute provider.
///
/// This runner exposes no WASI, environment, filesystem preopen, FIFO, raw
/// socket, or gateway authority. Every guest effect goes through the Bus
/// hostcall configured by Runtime.
pub struct ComponentProvider {
    instances: Arc<RwLock<HashMap<CapsuleId, ComponentActivation>>>,
    bus_hostcall: std::sync::RwLock<Option<BridgeHostcall>>,
    bridge_principals: Arc<RwLock<HashMap<CapsuleId, Option<String>>>>,
}

impl ComponentProvider {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            bus_hostcall: std::sync::RwLock::new(None),
            bridge_principals: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set_bus_hostcall(&self, hostcall: BridgeHostcall) {
        *self.bus_hostcall.write().unwrap() = Some(hostcall);
    }

    pub async fn set_bridge_principal(&self, capsule_id: &CapsuleId, principal_id: Option<String>) {
        self.bridge_principals
            .write()
            .await
            .insert(capsule_id.clone(), principal_id);
    }

    pub async fn clear_bridge_principal(&self, capsule_id: &CapsuleId) {
        self.bridge_principals.write().await.remove(capsule_id);
    }

    fn engine() -> Result<Engine> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        Engine::new(&config).map_err(|err| {
            ElastosError::Compute(format!("Failed to configure component engine: {err}"))
        })
    }

    fn store_limits() -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(COMPONENT_MEMORY_SIZE)
            .instances(16)
            .tables(32)
            .memories(4)
            .trap_on_grow_failure(true)
            .build()
    }

    fn is_component_manifest(manifest: &CapsuleManifest) -> bool {
        manifest.runtime_abi.as_ref() == Some(&CapsuleRuntimeAbi::ElastosComponentV1)
    }

    fn instantiate_component(
        engine: &Engine,
        component: &Component,
        hostcall: Option<BridgeHostcall>,
        capsule_id: String,
        principal_id: Option<String>,
    ) -> Result<()> {
        let mut store = Store::new(
            engine,
            ComponentState {
                hostcall,
                capsule_id,
                principal_id,
                limits: Self::store_limits(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(COMPONENT_FUEL)
            .map_err(|err| ElastosError::Compute(format!("Failed to set component fuel: {err}")))?;

        let mut linker = Linker::new(engine);
        ProductCapsuleV1::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|err| ElastosError::Compute(format!("Failed to link bus imports: {err}")))?;
        let bindings =
            ProductCapsuleV1::instantiate(&mut store, component, &linker).map_err(|err| {
                ElastosError::Compute(format!("Failed to instantiate component capsule: {err}"))
            })?;
        bindings
            .elastos_bus_lifecycle()
            .call_run(&mut store)
            .map_err(|err| ElastosError::Compute(format!("Component lifecycle run failed: {err}")))?
            .map_err(|err| ElastosError::Compute(format!("Component capsule denied: {err:?}")))?;

        Ok(())
    }
}

impl Default for ComponentProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ComputeProvider for ComponentProvider {
    async fn load(&self, path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
        if !Self::is_component_manifest(&manifest) {
            return Err(ElastosError::Compute(
                "ComponentProvider only accepts elastos.component/v1 manifests".into(),
            ));
        }

        let component_path = path.join(&manifest.entrypoint);
        if !component_path.exists() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "component file not found: {}",
                component_path.display()
            )));
        }

        let engine = Self::engine()?;
        let component = Component::from_file(&engine, &component_path).map_err(|err| {
            ElastosError::Compute(format!("Failed to compile component capsule: {err}"))
        })?;
        let id = CapsuleId::new(format!("component-{}", uuid::Uuid::new_v4()));

        self.instances.write().await.insert(
            id.clone(),
            ComponentActivation {
                engine,
                component,
                status: CapsuleStatus::Loading,
                manifest: manifest.clone(),
            },
        );

        Ok(CapsuleHandle {
            id,
            manifest,
            args: Vec::new(),
        })
    }

    async fn start(&self, handle: &CapsuleHandle) -> Result<()> {
        let (engine, component) = {
            let instances = self.instances.read().await;
            let instance = instances
                .get(&handle.id)
                .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;
            (instance.engine.clone(), instance.component.clone())
        };

        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(&handle.id) {
                instance.status = CapsuleStatus::Running;
            }
        }

        let hostcall = self.bus_hostcall.read().unwrap().clone();
        let principal_id = self
            .bridge_principals
            .read()
            .await
            .get(&handle.id)
            .cloned()
            .flatten();
        let capsule_id = handle.id.0.clone();
        let result = tokio::task::spawn_blocking(move || {
            Self::instantiate_component(&engine, &component, hostcall, capsule_id, principal_id)
        })
        .await
        .map_err(|err| ElastosError::Compute(format!("Component task join error: {err}")))?;

        self.instances.write().await.remove(&handle.id);

        result
    }

    async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        if self
            .instances
            .read()
            .await
            .get(&handle.id)
            .is_some_and(|instance| instance.status == CapsuleStatus::Running)
        {
            return Err(ElastosError::Compute(
                "component/v1 is a bounded activation contract and has no resident stop operation"
                    .to_string(),
            ));
        }
        self.instances.write().await.remove(&handle.id);
        Ok(())
    }

    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus> {
        self.instances
            .read()
            .await
            .get(&handle.id)
            .map(|instance| instance.status)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))
    }

    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
        let instances = self.instances.read().await;
        let instance = instances
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        Ok(CapsuleInfo {
            id: handle.id.clone(),
            name: instance.manifest.name.clone(),
            status: instance.status,
            memory_used_mb: 0,
        })
    }

    fn supports(&self, capsule_type: &CapsuleType) -> bool {
        matches!(capsule_type, CapsuleType::Wasm)
    }
}

fn bus_error(code: &str, message: impl Into<String>) -> elastos::bus::types::BusError {
    let message = message.into();
    match code {
        "denied" | "capability_denied" | "missing_token" | "invalid_token" => {
            elastos::bus::types::BusError::Denied(message)
        }
        "not_found" => elastos::bus::types::BusError::NotFound(message),
        "timeout" => elastos::bus::types::BusError::Timeout(message),
        "unavailable" => elastos::bus::types::BusError::Unavailable(message),
        "invalid" | "invalid_action" | "invalid_resource_invoke" => {
            elastos::bus::types::BusError::Invalid(message)
        }
        _ => elastos::bus::types::BusError::Internal(message),
    }
}

fn response_error(response: &RuntimeResponse) -> elastos::bus::types::BusError {
    match response {
        RuntimeResponse::Error { code, message } => bus_error(code, message.clone()),
        other => bus_error(
            "internal",
            format!("unexpected component bridge response: {other:?}"),
        ),
    }
}

fn json_body(
    bytes: &[u8],
) -> std::result::Result<serde_json::Value, elastos::bus::types::BusError> {
    if bytes.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(bytes).map_err(|err| {
        elastos::bus::types::BusError::Invalid(format!("component invoke body must be JSON: {err}"))
    })
}

impl ComponentState {
    fn call_bridge(
        &self,
        request: RuntimeRequest,
    ) -> std::result::Result<RuntimeResponse, elastos::bus::types::BusError> {
        let hostcall = self
            .hostcall
            .as_ref()
            .ok_or_else(|| bus_error("unavailable", "component bridge is not configured"))?;
        let line = guest_bus::bridge_envelope_json(1, request)
            .map_err(|err| bus_error("internal", err.to_string()))?;
        let raw = hostcall(&line, &self.capsule_id, self.principal_id.as_deref())
            .map_err(|err| bus_error("internal", err))?;
        let envelope: ResponseEnvelope =
            serde_json::from_str(&raw).map_err(|err| bus_error("internal", err.to_string()))?;
        if matches!(&envelope.response, RuntimeResponse::Error { .. }) {
            return Err(response_error(&envelope.response));
        }
        Ok(envelope.response)
    }
}

impl elastos::bus::types::Host for ComponentState {}

impl elastos::bus::runtime::Host for ComponentState {
    fn info(&mut self) -> elastos::bus::types::RuntimeInfo {
        elastos::bus::types::RuntimeInfo {
            abi: "elastos.component/v1".to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl elastos::bus::identity::Host for ComponentState {
    fn context(&mut self) -> elastos::bus::types::IdentityContext {
        elastos::bus::types::IdentityContext {
            principal: self.principal_id.clone(),
            capsule: self.capsule_id.clone(),
            session: self.capsule_id.clone(),
            device: None,
        }
    }
}

impl elastos::bus::capabilities::Host for ComponentState {
    fn request(
        &mut self,
        request: elastos::bus::types::CapabilityRequest,
    ) -> std::result::Result<elastos::bus::types::CapabilityGrant, elastos::bus::types::BusError>
    {
        if request.actions.len() != 1 {
            return Err(elastos::bus::types::BusError::Invalid(
                "component capability requests currently require exactly one action".into(),
            ));
        }
        let resource = request.resource;
        let action = request.actions[0].clone();
        let response = self.call_bridge(
            guest_bus::capability_request(guest_bus::CapabilityRequest {
                resource: resource.clone(),
                actions: vec![action.clone()],
                reason: request.reason,
            })
            .map_err(|err| bus_error("invalid", err))?,
        )?;
        let token = match response {
            RuntimeResponse::CapabilityToken { token } => token,
            response => return Err(response_error(&response)),
        };
        Ok(elastos::bus::types::CapabilityGrant {
            id: token,
            resource,
            actions: vec![action],
            expires_at_ms: None,
        })
    }
}

impl elastos::bus::providers::Host for ComponentState {
    fn invoke(
        &mut self,
        request: elastos::bus::types::InvokeRequest,
    ) -> std::result::Result<elastos::bus::types::InvokeResponse, elastos::bus::types::BusError>
    {
        let body = json_body(&request.body)?;
        let response = self.call_bridge(guest_bus::invoke_request(guest_bus::InvokeRequest {
            resource: request.resource,
            operation: request.operation,
            body,
            grant: request.grant,
        }))?;
        let (result, audit) = match response {
            RuntimeResponse::ResourceResult { result, audit } => (result, audit),
            response => return Err(response_error(&response)),
        };
        let status = result
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("ok")
            .to_string();
        let body = serde_json::to_vec(&result).map_err(|err| {
            bus_error(
                "internal",
                format!("failed to encode provider result: {err}"),
            )
        })?;
        Ok(elastos::bus::types::InvokeResponse {
            status,
            body,
            audit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn json_line(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("captured bridge line must be JSON")
    }

    #[test]
    fn component_provider_supports_only_wasm_type() {
        let provider = ComponentProvider::new();
        assert!(provider.supports(&CapsuleType::Wasm));
        assert!(!provider.supports(&CapsuleType::MicroVM));
    }

    #[test]
    fn provider_invoke_uses_runtime_bridge() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_host = captured.clone();
        let mut state = ComponentState {
            hostcall: Some(Arc::new(move |line, _capsule, _principal| {
                captured_host.lock().unwrap().push(line.to_string());
                Ok(serde_json::json!({
                    "id": 1,
                    "response": {
                        "type": "resource_result",
                        "result": { "status": "ok", "data": { "ok": true } },
                        "audit": "runtime-audit-1"
                    }
                })
                .to_string())
            })),
            capsule_id: "capsule-1".into(),
            principal_id: Some("person:local:test".into()),
            limits: ComponentProvider::store_limits(),
        };

        let result = elastos::bus::providers::Host::invoke(
            &mut state,
            elastos::bus::types::InvokeRequest {
                resource: "localhost://Users/self/Documents/test.json".into(),
                operation: "read".into(),
                body: b"{}".to_vec(),
                grant: Some("grant-token".into()),
            },
        )
        .unwrap();

        assert_eq!(result.status, "ok");
        assert_eq!(result.audit.as_deref(), Some("runtime-audit-1"));
        let line = captured.lock().unwrap().pop().unwrap();
        let expected = guest_bus::bridge_envelope_json(
            1,
            guest_bus::invoke_request(guest_bus::InvokeRequest {
                resource: "localhost://Users/self/Documents/test.json".into(),
                operation: "read".into(),
                body: serde_json::json!({}),
                grant: Some("grant-token".into()),
            }),
        )
        .unwrap();
        assert_eq!(json_line(&line), json_line(&expected));
    }

    #[test]
    fn provider_invoke_maps_runtime_denial_without_altering_request_shape() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_host = captured.clone();
        let mut state = ComponentState {
            hostcall: Some(Arc::new(move |line, _capsule, _principal| {
                captured_host.lock().unwrap().push(line.to_string());
                Ok(serde_json::json!({
                    "id": 1,
                    "response": {
                        "type": "error",
                        "code": "capability_denied",
                        "message": "Capability validation failed"
                    }
                })
                .to_string())
            })),
            capsule_id: "capsule-1".into(),
            principal_id: Some("person:local:test".into()),
            limits: ComponentProvider::store_limits(),
        };

        let err = elastos::bus::providers::Host::invoke(
            &mut state,
            elastos::bus::types::InvokeRequest {
                resource: "localhost://Users/self/Documents/test.json".into(),
                operation: "read".into(),
                body: b"{}".to_vec(),
                grant: Some("wrong-action-token".into()),
            },
        )
        .unwrap_err();

        assert!(matches!(err, elastos::bus::types::BusError::Denied(_)));
        let line = captured.lock().unwrap().pop().unwrap();
        let expected = guest_bus::bridge_envelope_json(
            1,
            guest_bus::provider_invoke(
                "localhost://Users/self/Documents/test.json",
                "read",
                serde_json::json!({}),
                Some("wrong-action-token".into()),
            ),
        )
        .unwrap();
        assert_eq!(json_line(&line), json_line(&expected));
    }

    #[test]
    fn capability_request_preserves_requested_resource() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_host = captured.clone();
        let mut state = ComponentState {
            hostcall: Some(Arc::new(move |line, _capsule, _principal| {
                captured_host.lock().unwrap().push(line.to_string());
                Ok(serde_json::json!({
                    "id": 1,
                    "response": {
                        "type": "capability_token",
                        "token": "grant-token"
                    }
                })
                .to_string())
            })),
            capsule_id: "capsule-1".into(),
            principal_id: None,
            limits: ComponentProvider::store_limits(),
        };

        let grant = elastos::bus::capabilities::Host::request(
            &mut state,
            elastos::bus::types::CapabilityRequest {
                resource: "localhost://Users/self/Documents/test.json".into(),
                actions: vec!["read".into()],
                reason: "test".into(),
            },
        )
        .unwrap();

        assert_eq!(grant.id, "grant-token");
        assert_eq!(grant.resource, "localhost://Users/self/Documents/test.json");
        assert_eq!(grant.actions, vec!["read"]);
        let line = captured.lock().unwrap().pop().unwrap();
        let expected = guest_bus::bridge_envelope_json(
            1,
            guest_bus::capability_request(guest_bus::CapabilityRequest {
                resource: "localhost://Users/self/Documents/test.json".into(),
                actions: vec!["read".into()],
                reason: "test".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(json_line(&line), json_line(&expected));
    }
}
