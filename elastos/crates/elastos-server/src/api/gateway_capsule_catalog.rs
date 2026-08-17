use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{AffordanceApprovalMode, AffordanceRisk, CapsuleAffordanceDescriptor};
use serde::{Deserialize, Serialize};

use super::*;

#[path = "gateway_capsule_catalog/bindings.rs"]
mod bindings;
#[path = "gateway_capsule_catalog/contract_audit.rs"]
mod contract_audit;
#[path = "gateway_capsule_catalog/read_model.rs"]
mod read_model;

use bindings::{
    resolve_capsule_method_binding, static_capsule_method_binding, RuntimeCapsuleAffordanceBinding,
};
pub(super) use read_model::{
    capsule_catalog_summary, capsule_interface_registry_summary, CapsuleCatalogResponse,
    CapsuleInterfaceRegistryResponse,
};

const CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA: &str = "elastos.capsules.invoke-result/v1";

pub(super) async fn capsule_catalog(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(capsule_catalog_summary(&state.data_dir)).into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(super) async fn capsule_interfaces(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(
            capsule_interface_registry_summary_with_bindings(
                &state.data_dir,
                state.provider_registry.as_deref(),
            )
            .await,
        )
        .into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(super) async fn capsule_interface_registry_summary_with_bindings(
    data_dir: &std::path::Path,
    registry: Option<&elastos_runtime::provider::ProviderRegistry>,
) -> read_model::CapsuleInterfaceRegistryResponse {
    let mut summary = capsule_interface_registry_summary(data_dir);
    for entry in &mut summary.interfaces {
        let mut bindings = Vec::with_capacity(entry.interface.methods.len());
        for method in &entry.interface.methods {
            bindings.push(
                resolve_capsule_method_binding(method, registry)
                    .await
                    .summary,
            );
        }
        entry.bindings = bindings;
    }
    summary.counts.executable_methods = summary
        .interfaces
        .iter()
        .flat_map(|entry| entry.bindings.iter())
        .filter(|binding| binding.executable)
        .count();
    summary
}

pub(super) fn capsule_affordance_static_executable(method: &CapsuleAffordanceDescriptor) -> bool {
    static_capsule_method_binding(method).summary.executable
}

pub(super) async fn capsule_contract_audit(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        Ok(_) => {
            let report = contract_audit::capsule_contract_audit_summary(&state).await;
            let status = if report.ok {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            };
            (status, Json(report)).into_response()
        }
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

pub(super) async fn capsule_interface_invoke(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<CapsuleInterfaceInvokeRequest>,
) -> Response {
    let resolved = match resolve_capsule_affordance(&state.data_dir, &request) {
        Ok(resolved) => resolved,
        Err(err) => {
            return capsule_invoke_request_error(
                &request,
                StatusCode::BAD_REQUEST,
                "affordance_not_declared",
                &err.to_string(),
            )
        }
    };

    let allowed_app = resolved.capsule.clone();
    let (caller_app, context) = match require_home_launch_token_for_any_app_context(
        &state.data_dir,
        &headers,
        &[allowed_app.as_str()],
    ) {
        Ok(value) => value,
        Err(err) => {
            return capsule_invoke_request_error(
                &request,
                StatusCode::FORBIDDEN,
                "forbidden",
                &err.to_string(),
            )
        }
    };

    let request_id = request.request_id.trim();
    if request_id.is_empty()
        || request_id.len() > 160
        || !request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
    {
        return capsule_invoke_error(
            &resolved,
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_request_id",
            "request_id must be a stable ASCII correlation id",
        );
    }
    let request_binding = crate::esp_binding::esp_request_binding(
        request_id,
        &context.principal_id,
        &resolved.capsule,
        Some(&resolved.interface_id),
        &resolved.method.id,
        resolved.method.resource.iter().cloned(),
        &request.input,
    );
    let binding =
        resolve_capsule_method_binding(&resolved.method, state.provider_registry.as_deref()).await;
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "requested",
            reason: &format!(
                "{} requested {} through {}",
                caller_app, resolved.method.id, resolved.interface_id
            ),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
            request_id,
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_failed",
            &err.to_string(),
        );
    }

    let runtime_binding = match (binding.summary.executable, binding.runtime_binding) {
        (true, Some(runtime_binding)) => runtime_binding,
        _ => {
            let approval_required = binding.summary.state == "approval-required";
            let status = if approval_required {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::NOT_IMPLEMENTED
            };
            let code = if approval_required {
                "approval_required"
            } else {
                "affordance_not_bound"
            };
            let message = binding
                .summary
                .reason
                .as_deref()
                .unwrap_or("method is not executable through generic interface invocation");
            let _ = append_provider_effect_audit(
                &state.data_dir,
                ProviderEffectAuditInput {
                    capsule_id: &resolved.capsule,
                    event_type: "capsule.affordance.failed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id,
                    result: "failed",
                    reason: message,
                },
            );
            return capsule_invoke_error(&resolved, request_id, status, code, message);
        }
    };
    let output =
        match dispatch_capsule_affordance(&state, &context, &request, runtime_binding).await {
            Ok(output) => output,
            Err((status, code, message)) => {
                let _ = append_provider_effect_audit(
                    &state.data_dir,
                    ProviderEffectAuditInput {
                        capsule_id: &resolved.capsule,
                        event_type: "capsule.affordance.failed",
                        principal_id: &context.principal_id,
                        session_id: &context.session_id,
                        request_id,
                        result: "failed",
                        reason: &message,
                    },
                );
                return capsule_invoke_error(&resolved, request_id, status, code, &message);
            }
        };

    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result: "completed",
            reason: &format!("Runtime completed {}", resolved.method.id),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
            request_id,
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_failed",
            &err.to_string(),
        );
    }

    Json(CapsuleInterfaceInvokeResponse {
        schema: CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA.to_string(),
        status: "ok".to_string(),
        capsule: resolved.capsule,
        interface: resolved.interface_id,
        method: resolved.method.id,
        request_id: request_id.to_string(),
        request_binding,
        output,
    })
    .into_response()
}

pub(super) fn require_capsule_catalog_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let allowed_apps = capsule_catalog_allowed_apps(data_dir);
    let allowed_refs = allowed_apps.iter().map(String::as_str).collect::<Vec<_>>();
    require_home_launch_token_for_any_context(data_dir, headers, &allowed_refs)
}

fn capsule_catalog_allowed_apps(data_dir: &std::path::Path) -> Vec<String> {
    let mut allowed = BTreeSet::from([
        HOME_CAPSULE_ID.to_string(),
        MARKETPLACE_CAPSULE_ID.to_string(),
        SYSTEM_CAPSULE_ID.to_string(),
    ]);
    for capsule in capsule_catalog_summary(data_dir).capsules {
        if capsule.role == CapsuleRole::Shell && capsule.launchable {
            allowed.insert(capsule.name);
        }
    }
    allowed.into_iter().collect()
}

fn resolve_capsule_affordance(
    data_dir: &std::path::Path,
    request: &CapsuleInterfaceInvokeRequest,
) -> anyhow::Result<ResolvedCapsuleAffordance> {
    let capsule_name = request.capsule.trim();
    let interface_id = request.interface.trim();
    let method_id = request.method.trim();
    if capsule_name.is_empty() || interface_id.is_empty() || method_id.is_empty() {
        anyhow::bail!("capsule, interface, and method are required");
    }

    let catalog = capsule_catalog_summary(data_dir);
    let capsule = catalog
        .capsules
        .iter()
        .find(|candidate| candidate.name == capsule_name)
        .ok_or_else(|| anyhow::anyhow!("capsule not found: {}", capsule_name))?;
    let interface = capsule
        .interfaces
        .iter()
        .find(|candidate| candidate.id == interface_id)
        .ok_or_else(|| anyhow::anyhow!("interface not declared: {}", interface_id))?;
    let method = interface
        .methods
        .iter()
        .find(|candidate| candidate.id == method_id)
        .ok_or_else(|| anyhow::anyhow!("method not declared: {}", method_id))?;

    Ok(ResolvedCapsuleAffordance {
        capsule: capsule.name.clone(),
        interface_id: interface.id.clone(),
        method: method.clone(),
    })
}

fn affordance_invocation_policy(
    method: &CapsuleAffordanceDescriptor,
) -> Result<(), (StatusCode, &'static str, &'static str)> {
    if method.approval == AffordanceApprovalMode::User {
        return Err((
            StatusCode::FORBIDDEN,
            "approval_required",
            "user-approved affordance invocation is not enabled yet",
        ));
    }
    if matches!(
        method.risk,
        AffordanceRisk::Payment
            | AffordanceRisk::Rights
            | AffordanceRisk::Actuator
            | AffordanceRisk::Privileged
    ) {
        return Err((
            StatusCode::FORBIDDEN,
            "approval_required",
            "high-risk affordances require explicit user approval before invocation",
        ));
    }
    Ok(())
}

async fn dispatch_capsule_affordance(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request: &CapsuleInterfaceInvokeRequest,
    binding: RuntimeCapsuleAffordanceBinding,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    match binding {
        RuntimeCapsuleAffordanceBinding::CatalogList => {
            serde_json::to_value(capsule_catalog_summary(&state.data_dir))
                .map(|catalog| serde_json::json!({ "catalog": catalog }))
                .map_err(|err| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "serialization_failed",
                        err.to_string(),
                    )
                })
        }
        RuntimeCapsuleAffordanceBinding::CapsuleLaunch => {
            dispatch_capsule_launch_affordance(state, context, request).await
        }
    }
}

async fn dispatch_capsule_launch_affordance(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    request: &CapsuleInterfaceInvokeRequest,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    let target = request
        .input
        .get("target")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if target.is_empty() || target == HOME_CAPSULE_ID {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "invalid Home target".to_string(),
        ));
    }

    let catalog = capsule_catalog_summary(&state.data_dir);
    let catalog_target = catalog
        .capsules
        .iter()
        .find(|capsule| capsule.name == target || capsule.launch_target.as_deref() == Some(target))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "target_not_found",
                "launch target not found".to_string(),
            )
        })?;
    if !catalog_target.launchable {
        return Err((
            StatusCode::FORBIDDEN,
            "target_not_launchable",
            "target is not installed and launchable".to_string(),
        ));
    }

    let target_name = catalog_target
        .launch_target
        .as_deref()
        .unwrap_or(catalog_target.name.as_str());
    let target_summary = home_launch_target(&state.data_dir, target_name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "target_not_found",
            "Home launch target not found".to_string(),
        )
    })?;
    let launch =
        launch_runtime_backed_home_target(&state.data_dir, target_summary.target.as_str(), context)
            .await;
    if let Some(message) = runtime_launch_failure(launch.as_ref()) {
        return Err((StatusCode::BAD_GATEWAY, "runtime_launch_failed", message));
    }
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
        target_summary
            .viewer
            .as_deref()
            .unwrap_or(target_summary.target.as_str()),
        &BTreeMap::new(),
        context,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "launch_token_failed",
            err.to_string(),
        )
    })?;

    Ok(serde_json::json!({
        "target": target_summary.target,
        "title": target_summary.title,
        "route": route,
        "attach_kind": target_summary.attach_kind,
        "role": target_summary.role,
        "target_kind": target_summary.target_kind,
        "launch_status": launch.as_ref().map(|summary| summary.status.clone()),
        "launch_detail": launch.as_ref().and_then(|summary| summary.detail.clone()),
        "capsule_id": launch.and_then(|summary| summary.capsule_id),
    }))
}

fn runtime_launch_failure(launch: Option<&GatewayRuntimeLaunchOutcome>) -> Option<String> {
    let failed = launch.filter(|launch| launch.status == "failed")?;
    Some(
        failed
            .detail
            .clone()
            .unwrap_or_else(|| "Runtime launch failed".to_string()),
    )
}

fn capsule_invoke_error(
    resolved: &ResolvedCapsuleAffordance,
    request_id: &str,
    status: StatusCode,
    code: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "schema": CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA,
            "status": "error",
            "code": code,
            "message": message,
            "capsule": resolved.capsule,
            "interface": resolved.interface_id,
            "method": resolved.method.id,
            "request_id": request_id,
        })),
    )
        .into_response()
}

fn capsule_invoke_request_error(
    request: &CapsuleInterfaceInvokeRequest,
    status: StatusCode,
    code: &str,
    message: &str,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "schema": CAPSULE_INTERFACE_INVOKE_RESULT_SCHEMA,
            "status": "error",
            "code": code,
            "message": message,
            "capsule": request.capsule,
            "interface": request.interface,
            "method": request.method,
            "request_id": request.request_id,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapsuleInterfaceInvokeRequest {
    request_id: String,
    capsule: String,
    interface: String,
    method: String,
    #[serde(default)]
    input: serde_json::Value,
}

#[derive(Serialize)]
struct CapsuleInterfaceInvokeResponse {
    schema: String,
    status: String,
    capsule: String,
    interface: String,
    method: String,
    request_id: String,
    request_binding: crate::esp_binding::EspRequestBinding,
    output: serde_json::Value,
}

#[derive(Debug)]
struct ResolvedCapsuleAffordance {
    capsule: String,
    interface_id: String,
    method: CapsuleAffordanceDescriptor,
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::CapsuleExecution;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct BindingProvider {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl elastos_runtime::provider::Provider for BindingProvider {
        async fn handle(
            &self,
            _request: elastos_runtime::provider::ResourceRequest,
        ) -> Result<
            elastos_runtime::provider::ResourceResponse,
            elastos_runtime::provider::ProviderError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(elastos_runtime::provider::ResourceResponse::Ok)
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "binding-provider"
        }
    }

    fn activate_test_capsule(data_dir: &std::path::Path, name: &str) {
        let path = data_dir.join("components.json");
        let mut components = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_else(|| {
                serde_json::json!({
                    "external": {},
                    "capsules": {},
                    "profiles": {}
                })
            });
        components["external"][name] = serde_json::json!({
            "install_path": format!("capsules/{name}"),
            "platforms": {}
        });
        fs::write(&path, serde_json::to_vec_pretty(&components).unwrap()).unwrap();
    }

    fn copy_test_tree(source: &std::path::Path, target: &std::path::Path) {
        if source.is_dir() {
            fs::create_dir_all(target).unwrap();
            for entry in fs::read_dir(source).unwrap().flatten() {
                copy_test_tree(&entry.path(), &target.join(entry.file_name()));
            }
        } else {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(source, target).unwrap();
        }
    }

    fn install_active_first_party_capsules(data_dir: &std::path::Path) {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let components: crate::setup::ComponentsManifest =
            serde_json::from_slice(&fs::read(repo.join("components.json")).unwrap()).unwrap();
        let registered = components
            .external
            .keys()
            .chain(components.capsules.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        fs::copy(
            repo.join("components.json"),
            data_dir.join("components.json"),
        )
        .unwrap();
        for manifest in crate::api::capsule_inventory::list_development_capsule_manifests() {
            if !registered.contains(&manifest.name) {
                continue;
            }
            activate_test_capsule(data_dir, &manifest.name);
            let source = repo.join("capsules").join(&manifest.name);
            let target = data_dir.join("capsules").join(&manifest.name);
            copy_test_tree(&source.join("capsule.json"), &target.join("capsule.json"));
            if source.join("browser").is_dir() {
                copy_test_tree(&source.join("browser"), &target.join("browser"));
            }
            if source.join(&manifest.entrypoint).is_file() {
                copy_test_tree(
                    &source.join(&manifest.entrypoint),
                    &target.join(&manifest.entrypoint),
                );
            }
        }
    }

    #[tokio::test]
    async fn capsule_contract_audit_requires_a_system_launch_token() {
        let data_dir = tempfile::tempdir().unwrap();
        let state = GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: data_dir.path().to_path_buf(),
            data_dir: data_dir.path().to_path_buf(),
            audit_log: Arc::new(std::sync::OnceLock::new()),
        };
        let response = capsule_contract_audit(State(state), HeaderMap::new()).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    fn write_capsule(data_dir: &std::path::Path, name: &str, role: &str, capsule_type: &str) {
        activate_test_capsule(data_dir, name);
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
        let entrypoint = match capsule_type {
            "wasm" => format!("{name}.wasm"),
            "microvm" => "rootfs.ext4".to_string(),
            _ => "index.html".to_string(),
        };
        let mut manifest = serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": format!("{name} test capsule"),
            "author": "elastos",
            "role": role,
            "type": capsule_type,
            "entrypoint": entrypoint,
            "signature": "test-signature"
        });
        if role == "provider" {
            manifest["provides"] = serde_json::json!(format!("elastos://{name}/*"));
            manifest["authority"] = serde_json::json!({
                "reason": "Test provider boundary",
                "capabilities": [{
                    "resource": format!("elastos://{name}/*"),
                    "actions": ["read"],
                    "operations": ["status"]
                }],
                "audit_events": [format!("{name}.status")]
            });
        }
        fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        if capsule_type == "wasm" {
            fs::write(dir.join(format!("{name}.wasm")), b"\0asm").unwrap();
            fs::create_dir_all(dir.join("browser")).unwrap();
            fs::write(dir.join("browser/index.html"), "<!doctype html>").unwrap();
        } else {
            fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        }
    }

    fn write_capsule_json(data_dir: &std::path::Path, name: &str, manifest: serde_json::Value) {
        activate_test_capsule(data_dir, name);
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        if manifest
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|capsule_type| capsule_type == "wasm")
        {
            fs::write(dir.join(format!("{name}.wasm")), b"\0asm").unwrap();
            fs::create_dir_all(dir.join("browser")).unwrap();
            fs::write(dir.join("browser").join("index.html"), "<!doctype html>").unwrap();
        } else {
            fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        }
    }

    #[test]
    fn capsule_catalog_lists_roles_and_launchable_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule(data_dir.path(), "home", "app", "wasm");
        write_capsule(data_dir.path(), "home-gui", "shell", "wasm");
        write_capsule(data_dir.path(), "home-cli", "shell", "wasm");
        write_capsule(data_dir.path(), "marketplace", "app", "wasm");
        write_capsule(data_dir.path(), "documents", "viewer", "wasm");
        write_capsule(data_dir.path(), "object-provider", "provider", "microvm");

        let catalog = capsule_catalog_summary(data_dir.path());

        assert_eq!(catalog.schema, read_model::CAPSULE_CATALOG_SCHEMA);
        assert!(catalog.counts.total >= 3);
        assert!(catalog.counts.apps >= 1);
        assert!(catalog.counts.viewers >= 1);
        assert!(catalog.counts.providers >= 1);
        assert!(catalog.counts.shell >= 1);
        let home = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "home")
            .unwrap();
        assert!(!home.launchable);
        assert_eq!(home.role, CapsuleRole::App);
        assert_eq!(home.route.as_deref(), None);
        assert_eq!(home.projection.schema, "elastos.capsule.projection/v1");
        assert_eq!(home.projection.web.state, "not-launchable");
        assert_eq!(home.projection.web.route.as_deref(), None);
        assert_eq!(home.projection.cli.state, "facts-only");
        assert_eq!(home.projection.facts.state, "available");
        let launchable_shells = catalog
            .capsules
            .iter()
            .filter(|capsule| capsule.role == CapsuleRole::Shell && capsule.launchable)
            .map(|capsule| capsule.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(launchable_shells, vec!["home-cli", "home-gui"]);
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert!(marketplace.launchable);
        assert_eq!(marketplace.trust_state, "local-manifest-signature");
        assert_eq!(marketplace.projection.web.state, "available");
        let provider = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "object-provider")
            .unwrap();
        assert!(!provider.launchable);
        assert_eq!(provider.title, "Storage");
        assert_eq!(provider.projection.web.state, "provider-only");
        assert_eq!(provider.projection.carrier.state, "service-endpoint");
        assert!(provider.repository.is_none());
    }

    #[test]
    fn capsule_interface_registry_lists_declared_affordances() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "marketplace",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace test capsule",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm",
                "signature": "test-signature",
                "interfaces": [{
                    "id": "elastos.marketplace.catalog",
                    "version": "0.1.0",
                    "methods": [
                        {
                            "id": "catalog.list",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://capsules/*",
                            "operation": "list"
                        },
                        {
                            "id": "capsule.open",
                            "risk": "launch",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://capsules/*",
                            "operation": "launch"
                        }
                    ]
                }]
            }),
        );
        let catalog = capsule_catalog_summary(data_dir.path());
        assert!(catalog.counts.interfaces >= 1);
        assert!(catalog.counts.methods >= 2);
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert_eq!(marketplace.interfaces[0].id, "elastos.marketplace.catalog");
        assert_eq!(marketplace.interfaces.len(), 1);
        assert_eq!(marketplace.interfaces[0].methods.len(), 2);
        assert_eq!(marketplace.projection.affordances.state, "declared");
        assert_eq!(marketplace.projection.gates.state, "declared");
        assert_eq!(marketplace.projection.audit_mirror.state, "redacted");
        assert!(marketplace
            .projection
            .cli
            .schemas
            .contains(&read_model::CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string()));
        let serialized = serde_json::to_value(&catalog).unwrap();
        let marketplace_json = serialized["capsules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|capsule| capsule["name"] == "marketplace")
            .unwrap();
        assert_eq!(
            marketplace_json["projection"]["schema"],
            "elastos.capsule.projection/v1"
        );
        assert_eq!(
            marketplace_json["projection"]["affordances"]["state"],
            "declared"
        );

        let registry = capsule_interface_registry_summary(data_dir.path());
        assert_eq!(
            registry.schema,
            read_model::CAPSULE_INTERFACE_REGISTRY_SCHEMA
        );
        assert!(registry.counts.capsules >= 1);
        assert!(registry.counts.interfaces >= 1);
        assert!(registry.counts.methods >= 2);
        assert!(registry.counts.executable_methods >= 2);
        let marketplace_registry = registry
            .interfaces
            .iter()
            .find(|interface| interface.capsule == "marketplace")
            .unwrap();
        assert_eq!(marketplace_registry.interface.methods[1].id, "capsule.open");
        assert_eq!(marketplace_registry.bindings.len(), 2);
        for (method, binding) in marketplace_registry
            .interface
            .methods
            .iter()
            .zip(&marketplace_registry.bindings)
        {
            assert_eq!(binding.method, method.id);
            assert!(binding.executable);
            assert_eq!(binding.handler_kind.as_deref(), Some("runtime"));
        }
        assert_eq!(registry.policy.invocation_state, "runtime-gated");
    }

    #[test]
    fn first_party_capsules_have_complete_projection_contract() {
        let data_dir = tempfile::tempdir().unwrap();
        install_active_first_party_capsules(data_dir.path());
        let manifests = crate::api::capsule_inventory::list_development_capsule_manifests();
        let manifest_names = manifests
            .iter()
            .map(|manifest| manifest.name.as_str())
            .collect::<BTreeSet<_>>();
        assert!(
            manifest_names.len() >= 30,
            "expected the first-party development capsule set to be visible"
        );

        let catalog = capsule_catalog_summary(data_dir.path());
        assert_eq!(catalog.schema, read_model::CAPSULE_CATALOG_SCHEMA);
        assert_eq!(catalog.counts.total, catalog.capsules.len());
        let catalog_names = catalog
            .capsules
            .iter()
            .map(|capsule| capsule.name.as_str())
            .collect::<BTreeSet<_>>();
        let active = crate::api::capsule_inventory::active_capsule_names(data_dir.path()).unwrap();
        let active_names = active.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let expected = manifest_names
            .intersection(&active_names)
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(catalog_names, expected);
        assert!(!catalog_names.contains("bus-v1-conformance"));

        let catalog_interface_count = catalog
            .capsules
            .iter()
            .map(|capsule| capsule.interfaces.len())
            .sum::<usize>();
        let catalog_method_count = catalog
            .capsules
            .iter()
            .flat_map(|capsule| capsule.interfaces.iter())
            .map(|interface| interface.methods.len())
            .sum::<usize>();
        assert_eq!(catalog.counts.interfaces, catalog_interface_count);
        assert_eq!(catalog.counts.methods, catalog_method_count);

        for required in [
            HOME_CAPSULE_ID,
            "home-gui",
            "home-cli",
            "browser",
            "wallet",
            "inbox",
            "services",
            SYSTEM_CAPSULE_ID,
            "library",
            "documents",
            "object-provider",
            "wallet-provider",
            "net-provider",
            "exit-provider",
        ] {
            assert!(
                catalog_names.contains(required),
                "required first-party capsule {required} was missing"
            );
        }
        let launchable_shells = catalog
            .capsules
            .iter()
            .filter(|capsule| capsule.role == CapsuleRole::Shell && capsule.launchable)
            .map(|capsule| capsule.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(launchable_shells, vec!["home-cli", "home-gui"]);
        let home_gui = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "home-gui")
            .expect("home-gui must remain a first-party shell identity");
        assert_eq!(home_gui.execution, Some(CapsuleExecution::WebProjection));
        assert_eq!(
            home_gui.bus_contract.as_deref(),
            Some("elastos.runtime-projection/v1")
        );
        let gba = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "gba-emulator")
            .expect("installed active GBA viewer must remain in the product catalog");
        assert_eq!(gba.role, CapsuleRole::Viewer);
        assert!(gba.installed);
        assert!(gba.launchable);
        assert_eq!(gba.accepted_content.len(), 1);
        assert_eq!(gba.accepted_content[0].name, "gba-ucity");
        let ucity = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "gba-ucity")
            .expect("installed active uCity demo must remain in the product catalog");
        assert_eq!(ucity.role, CapsuleRole::Content);
        assert_eq!(ucity.viewer.as_deref(), Some("gba-emulator"));
        assert!(ucity.launchable);
        assert_eq!(ucity.launch_target.as_deref(), Some("gba-ucity"));

        let by_name = catalog
            .capsules
            .iter()
            .map(|capsule| (capsule.name.as_str(), capsule))
            .collect::<BTreeMap<_, _>>();
        for (name, role) in [
            ("browser", CapsuleRole::App),
            ("wallet", CapsuleRole::App),
            ("documents", CapsuleRole::Viewer),
            ("archive-manager", CapsuleRole::Viewer),
            ("home-cli", CapsuleRole::Shell),
            ("home-gui", CapsuleRole::Shell),
            ("object-provider", CapsuleRole::Provider),
            ("wallet-provider", CapsuleRole::Provider),
            ("net-provider", CapsuleRole::Provider),
            ("exit-provider", CapsuleRole::Provider),
        ] {
            assert_eq!(by_name[name].role, role, "wrong canonical role for {name}");
        }
        assert_eq!(
            by_name["browser"]
                .requires
                .iter()
                .map(|requirement| requirement.name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "browser-engine-adapter",
                "exit-provider",
                "net-provider",
                "wallet-provider",
            ])
        );
        assert_eq!(
            by_name["gba-emulator"]
                .requires
                .iter()
                .map(|requirement| requirement.name.as_str())
                .collect::<Vec<_>>(),
            vec!["object-provider"]
        );
        assert_eq!(by_name["chat"].projection.cli.state, "available");
        assert_eq!(by_name["browser"].projection.cli.state, "facts-only");

        let home_targets = home_targets_from_catalog(&catalog);
        let target_names = home_targets
            .iter()
            .map(|target| target.target.as_str())
            .collect::<BTreeSet<_>>();
        let expected_targets = catalog
            .capsules
            .iter()
            .filter(|capsule| capsule.launchable)
            .filter(|capsule| capsule.role != CapsuleRole::Shell)
            .filter(|capsule| is_home_visible_target(&capsule.name))
            .map(|capsule| capsule.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(target_names, expected_targets);
        for target in &home_targets {
            let capsule = by_name[target.target.as_str()];
            assert_eq!(target.title, capsule.title);
            assert_eq!(target.description, capsule.description);
            assert_eq!(target.route.as_str(), capsule.route.as_deref().unwrap());
            assert_eq!(target.role, capsule.role);
            assert_eq!(target.viewer, capsule.viewer);
            assert_eq!(target.viewer_title, capsule.viewer_title);
        }

        for capsule in &catalog.capsules {
            let projection = &capsule.projection;
            assert_eq!(projection.schema, "elastos.capsule.projection/v1");
            for (surface_name, surface) in [
                ("web", &projection.web),
                ("cli", &projection.cli),
                ("facts", &projection.facts),
                ("affordances", &projection.affordances),
                ("gates", &projection.gates),
                ("audit_mirror", &projection.audit_mirror),
                ("carrier", &projection.carrier),
            ] {
                assert!(
                    !surface.state.trim().is_empty(),
                    "{} projection {} state was empty",
                    capsule.name,
                    surface_name
                );
                assert!(
                    !surface.source.trim().is_empty(),
                    "{} projection {} source was empty",
                    capsule.name,
                    surface_name
                );
            }
            assert!(projection
                .cli
                .schemas
                .contains(&read_model::CAPSULE_CATALOG_SCHEMA.to_string()));
            assert!(projection
                .cli
                .schemas
                .contains(&read_model::CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string()));
            assert_eq!(
                projection.facts.route.as_deref(),
                Some("/api/capsules/catalog")
            );
            assert!(projection
                .facts
                .schemas
                .contains(&"elastos.esp.initialize/v0".to_string()));
            assert_eq!(
                projection.affordances.route.as_deref(),
                Some("/api/capsules/interfaces")
            );
            assert_eq!(
                projection.gates.route.as_deref(),
                Some("/api/esp/initialize")
            );
            assert_eq!(projection.audit_mirror.state, "redacted");
            assert!(projection
                .audit_mirror
                .schemas
                .contains(&"elastos.inspect.object/v1".to_string()));

            if capsule.interfaces.is_empty() {
                assert_eq!(projection.affordances.state, "absent");
                assert_eq!(projection.gates.state, "absent");
            } else {
                assert_eq!(projection.affordances.state, "declared");
                assert_eq!(projection.gates.state, "declared");
            }
            if capsule.launchable {
                assert_eq!(projection.web.state, "available");
                assert!(
                    projection.web.route.is_some(),
                    "launchable capsule {} must expose a web route",
                    capsule.name
                );
            }
            if capsule.role == CapsuleRole::Provider || capsule.provides.is_some() {
                assert!(
                    matches!(
                        projection.carrier.state.as_str(),
                        "service-endpoint" | "requires-provider-intents"
                    ),
                    "provider capsule {} must project a Carrier/provider surface, got {}",
                    capsule.name,
                    projection.carrier.state
                );
            }
        }

        let registry = capsule_interface_registry_summary(data_dir.path());
        assert_eq!(
            registry.schema,
            read_model::CAPSULE_INTERFACE_REGISTRY_SCHEMA
        );
        assert_eq!(registry.counts.interfaces, catalog_interface_count);
        assert_eq!(registry.counts.methods, catalog_method_count);
        assert_eq!(
            registry.counts.capsules,
            registry
                .interfaces
                .iter()
                .map(|interface| interface.capsule.as_str())
                .collect::<BTreeSet<_>>()
                .len()
        );
        for interface in &registry.interfaces {
            assert!(
                catalog_names.contains(interface.capsule.as_str()),
                "interface registry referenced unknown capsule {}",
                interface.capsule
            );
            let capsule = by_name[interface.capsule.as_str()];
            assert_eq!(interface.capsule_version, capsule.version);
            assert_eq!(interface.title, capsule.title);
            assert_eq!(interface.role, capsule.role);
            assert_eq!(interface.capsule_type, capsule.capsule_type);
            assert_eq!(interface.runtime_abi, capsule.runtime_abi);
            assert_eq!(interface.execution, capsule.execution);
            assert_eq!(interface.trust_state, capsule.trust_state);
            assert!(
                !interface.interface.id.trim().is_empty(),
                "interface registry contained an empty interface id for {}",
                interface.capsule
            );
        }
        for (capsule_name, extension) in [
            ("gba-emulator", ".gba"),
            ("documents", ".md"),
            ("archive-manager", ".zip"),
        ] {
            assert!(registry
                .interfaces
                .iter()
                .filter(|interface| interface.capsule == capsule_name)
                .flat_map(|interface| interface.interface.methods.iter())
                .filter_map(|method| method.input_schema.as_ref())
                .any(|schema| schema.to_string().contains(extension)),
                "{capsule_name} must expose {extension} acceptance through the canonical interface registry");
        }
    }

    #[test]
    fn capsule_affordance_resolves_declared_descriptor() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "marketplace",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace test capsule",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm",
                "signature": "test-signature",
                "interfaces": [{
                    "id": "elastos.marketplace.catalog",
                    "version": "0.1.0",
                    "methods": [{
                        "id": "catalog.list",
                        "risk": "read",
                        "approval": "runtime_policy",
                        "audit": "event",
                        "resource": "elastos://capsules/*",
                        "operation": "list"
                    }]
                }]
            }),
        );

        let request = CapsuleInterfaceInvokeRequest {
            request_id: "test-request-1".to_string(),
            capsule: "marketplace".to_string(),
            interface: "elastos.marketplace.catalog".to_string(),
            method: "catalog.list".to_string(),
            input: serde_json::json!({}),
        };
        let resolved = resolve_capsule_affordance(data_dir.path(), &request).unwrap();
        assert_eq!(resolved.capsule, "marketplace");
        assert_eq!(resolved.interface_id, "elastos.marketplace.catalog");
        assert_eq!(resolved.method.id, "catalog.list");
    }

    #[tokio::test]
    async fn capsule_method_bindings_distinguish_runtime_provider_and_descriptive_methods() {
        let runtime_method = CapsuleAffordanceDescriptor {
            id: "capsule.open".to_string(),
            description: None,
            risk: AffordanceRisk::Launch,
            approval: AffordanceApprovalMode::RuntimePolicy,
            audit: elastos_common::AffordanceAuditMode::Event,
            resource: Some("elastos://capsules/*".to_string()),
            operation: Some("launch".to_string()),
            input_schema: None,
            output_schema: None,
        };
        let runtime = resolve_capsule_method_binding(&runtime_method, None).await;
        assert_eq!(runtime.summary.state, "executable");
        assert!(runtime.summary.handler_available);
        assert!(runtime.summary.executable);
        assert_eq!(
            runtime.summary.handler.as_deref(),
            Some("runtime.capsule.launch")
        );

        let descriptive = CapsuleAffordanceDescriptor {
            id: "capsule.describe".to_string(),
            resource: None,
            operation: None,
            ..runtime_method.clone()
        };
        let descriptive = resolve_capsule_method_binding(&descriptive, None).await;
        assert_eq!(descriptive.summary.state, "descriptive-only");
        assert!(!descriptive.summary.handler_available);
        assert!(!descriptive.summary.executable);

        let registry = elastos_runtime::provider::ProviderRegistry::new();
        registry
            .register_sub_provider("chain", Arc::new(BindingProvider::default()))
            .await
            .unwrap();
        let provider_method = CapsuleAffordanceDescriptor {
            id: "chain.status".to_string(),
            resource: Some("elastos://chain/*".to_string()),
            operation: Some("status".to_string()),
            ..runtime_method
        };
        let provider = resolve_capsule_method_binding(&provider_method, Some(&registry)).await;
        assert_eq!(provider.summary.state, "provider-path-only");
        assert!(provider.summary.handler_available);
        assert!(!provider.summary.executable);
        assert_eq!(
            provider.summary.handler.as_deref(),
            Some("binding-provider")
        );
        assert_eq!(provider.summary.required_action.as_deref(), Some("read"));
    }

    #[tokio::test]
    async fn generic_invoke_executes_runtime_binding_and_never_dispatches_provider_path() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule_json(
            data_dir.path(),
            "marketplace",
            serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace test capsule",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "runtime_abi": "elastos.runtime-projection/v1",
                "bus_contract": "elastos.runtime-projection/v1",
                "execution": "web-projection",
                "projections": ["web", "affordances"],
                "entrypoint": "browser/index.html",
                "interfaces": [{
                    "id": "elastos.marketplace.catalog",
                    "version": "0.1.0",
                    "methods": [
                        {
                            "id": "catalog.list",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://capsules/*",
                            "operation": "list"
                        },
                        {
                            "id": "chain.status",
                            "risk": "read",
                            "approval": "runtime_policy",
                            "audit": "event",
                            "resource": "elastos://chain/*",
                            "operation": "status"
                        }
                    ]
                }]
            }),
        );
        let provider = Arc::new(BindingProvider::default());
        let registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
        registry
            .register_sub_provider("chain", provider.clone())
            .await
            .unwrap();
        let state = GatewayState {
            provider_registry: Some(registry),
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: data_dir.path().to_path_buf(),
            data_dir: data_dir.path().to_path_buf(),
            audit_log: Arc::new(std::sync::OnceLock::new()),
        };
        let token = issue_home_launch_token(data_dir.path(), "marketplace").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", "localhost:61180".parse().unwrap());
        headers.insert("origin", "null".parse().unwrap());
        headers.insert("x-elastos-home-token", token.parse().unwrap());

        let runtime_response = capsule_interface_invoke(
            State(state.clone()),
            headers.clone(),
            Json(CapsuleInterfaceInvokeRequest {
                request_id: "test-runtime-request".to_string(),
                capsule: "marketplace".to_string(),
                interface: "elastos.marketplace.catalog".to_string(),
                method: "catalog.list".to_string(),
                input: serde_json::json!({}),
            }),
        )
        .await;
        assert_eq!(runtime_response.status(), StatusCode::OK);

        let provider_response = capsule_interface_invoke(
            State(state),
            headers,
            Json(CapsuleInterfaceInvokeRequest {
                request_id: "test-provider-request".to_string(),
                capsule: "marketplace".to_string(),
                interface: "elastos.marketplace.catalog".to_string(),
                method: "chain.status".to_string(),
                input: serde_json::json!({}),
            }),
        )
        .await;
        assert_eq!(provider_response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = axum::body::to_bytes(provider_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "affordance_not_bound");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn generic_invoke_propagates_runtime_launch_failure() {
        let launch = GatewayRuntimeLaunchOutcome {
            status: "failed".to_string(),
            capsule_id: None,
            detail: Some("compute provider rejected launch".to_string()),
        };
        assert_eq!(
            runtime_launch_failure(Some(&launch)).as_deref(),
            Some("compute provider rejected launch")
        );
        assert!(runtime_launch_failure(None).is_none());
    }

    #[test]
    fn capsule_interface_invoke_request_rejects_hidden_authority_fields() {
        let err = serde_json::from_value::<CapsuleInterfaceInvokeRequest>(serde_json::json!({
            "request_id": "test-hidden-authority",
            "capsule": "marketplace",
            "interface": "elastos.marketplace.catalog",
            "method": "catalog.list",
            "input": {},
            "principal_id": "person:other",
            "_runtime_invocation": {
                "schema": "forged"
            }
        }))
        .unwrap_err();

        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn capsule_affordance_policy_rejects_high_risk_without_approval_binding() {
        let resolved = ResolvedCapsuleAffordance {
            capsule: "wallet".to_string(),
            interface_id: "elastos.wallet.payment".to_string(),
            method: CapsuleAffordanceDescriptor {
                id: "payment.send".to_string(),
                description: None,
                risk: AffordanceRisk::Payment,
                approval: AffordanceApprovalMode::RuntimePolicy,
                audit: elastos_common::AffordanceAuditMode::Full,
                resource: Some("elastos://wallet/*".to_string()),
                operation: Some("send".to_string()),
                input_schema: None,
                output_schema: None,
            },
        };

        let err = affordance_invocation_policy(&resolved.method).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1, "approval_required");
    }
}
