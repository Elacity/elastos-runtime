use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{AffordanceApprovalMode, AffordanceRisk, CapsuleAffordanceDescriptor};
use serde::{Deserialize, Serialize};

use super::*;

#[path = "gateway_capsule_catalog/read_model.rs"]
mod read_model;

pub(super) use read_model::{capsule_catalog_summary, capsule_interface_registry_summary};

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
        Ok(_) => Json(capsule_interface_registry_summary(&state.data_dir)).into_response(),
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
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response()
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
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    };

    let request_id = format!(
        "capsule-affordance:{}:{}:{}",
        resolved.capsule,
        resolved.method.id,
        now_ts()
    );
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            reason: &format!(
                "{} requested {} through {}",
                caller_app, resolved.method.id, resolved.interface_id
            ),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_failed",
            &err.to_string(),
        );
    }

    let output = match enforce_affordance_invocation_policy(&resolved) {
        Ok(()) => match dispatch_capsule_affordance(&state, &context, &resolved, &request).await {
            Ok(output) => output,
            Err((status, code, message)) => {
                let _ = append_provider_effect_audit(
                    &state.data_dir,
                    ProviderEffectAuditInput {
                        capsule_id: &resolved.capsule,
                        event_type: "capsule.affordance.failed",
                        principal_id: &context.principal_id,
                        session_id: &context.session_id,
                        request_id: &request_id,
                        result: "failed",
                        reason: &message,
                    },
                );
                return capsule_invoke_error(&resolved, status, code, &message);
            }
        },
        Err((status, code, message)) => {
            let _ = append_provider_effect_audit(
                &state.data_dir,
                ProviderEffectAuditInput {
                    capsule_id: &resolved.capsule,
                    event_type: "capsule.affordance.failed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: &request_id,
                    result: "failed",
                    reason: message,
                },
            );
            return capsule_invoke_error(&resolved, status, code, message);
        }
    };

    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: &resolved.capsule,
            event_type: "capsule.affordance.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "completed",
            reason: &format!("Runtime completed {}", resolved.method.id),
        },
    ) {
        return capsule_invoke_error(
            &resolved,
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
        request_id,
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

fn enforce_affordance_invocation_policy(
    resolved: &ResolvedCapsuleAffordance,
) -> Result<(), (StatusCode, &'static str, &'static str)> {
    if resolved.method.approval == AffordanceApprovalMode::User {
        return Err((
            StatusCode::FORBIDDEN,
            "approval_required",
            "user-approved affordance invocation is not enabled yet",
        ));
    }
    if matches!(
        resolved.method.risk,
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
    resolved: &ResolvedCapsuleAffordance,
    request: &CapsuleInterfaceInvokeRequest,
) -> Result<serde_json::Value, (StatusCode, &'static str, String)> {
    let resource = resolved.method.resource.as_deref().unwrap_or_default();
    let operation = resolved.method.operation.as_deref().unwrap_or_default();
    match (resource, operation) {
        ("elastos://capsules/*", "list") => {
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
        ("elastos://capsules/*", "launch") => {
            dispatch_capsule_launch_affordance(state, context, request).await
        }
        _ => Err((
            StatusCode::NOT_IMPLEMENTED,
            "affordance_not_bound",
            format!(
                "{} is declared but not yet bound to a Runtime/provider handler",
                resolved.method.id
            ),
        )),
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
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
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

fn capsule_invoke_error(
    resolved: &ResolvedCapsuleAffordance,
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
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapsuleInterfaceInvokeRequest {
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
    use std::fs;

    fn write_capsule(data_dir: &std::path::Path, name: &str, role: &str, capsule_type: &str) {
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
        let entrypoint = match capsule_type {
            "wasm" => format!("{name}.wasm"),
            "microvm" => "rootfs.ext4".to_string(),
            _ => "index.html".to_string(),
        };
        fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "description": format!("{name} test capsule"),
                "author": "elastos",
                "role": role,
                "type": capsule_type,
                "entrypoint": entrypoint,
                "signature": "test-signature"
            }))
            .unwrap(),
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
        let marketplace_registry = registry
            .interfaces
            .iter()
            .find(|interface| interface.capsule == "marketplace")
            .unwrap();
        assert_eq!(marketplace_registry.interface.methods[1].id, "capsule.open");
        assert_eq!(registry.policy.invocation_state, "runtime-gated");
    }

    #[test]
    fn first_party_capsules_have_complete_projection_contract() {
        let data_dir = tempfile::tempdir().unwrap();
        let manifests = crate::api::capsule_inventory::list_capsule_manifests(data_dir.path());
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
        for name in &manifest_names {
            assert!(
                catalog_names.contains(name),
                "first-party capsule {name} was missing from the Runtime catalog"
            );
        }

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
        assert!(
            home_gui.description.contains("host-loaded"),
            "home-gui must be described as host-loaded until it has a true isolated shell attach path"
        );

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
            assert!(
                !interface.interface.id.trim().is_empty(),
                "interface registry contained an empty interface id for {}",
                interface.capsule
            );
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

    #[test]
    fn capsule_interface_invoke_request_rejects_hidden_authority_fields() {
        let err = serde_json::from_value::<CapsuleInterfaceInvokeRequest>(serde_json::json!({
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

        let err = enforce_affordance_invocation_policy(&resolved).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1, "approval_required");
    }
}
