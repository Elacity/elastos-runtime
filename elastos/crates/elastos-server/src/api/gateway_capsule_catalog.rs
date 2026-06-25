use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{
    AffordanceApprovalMode, AffordanceRisk, CapsuleAffordanceDescriptor,
    CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleRole, CapsuleType,
};
use serde::{Deserialize, Serialize};

use super::*;

const CAPSULE_CATALOG_SCHEMA: &str = "elastos.capsules.catalog/v1";
const CAPSULE_INTERFACE_REGISTRY_SCHEMA: &str = "elastos.capsules.interfaces/v1";
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
    require_home_launch_token_for_any_context(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID, MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    )
}

pub(super) fn capsule_catalog_summary(data_dir: &std::path::Path) -> CapsuleCatalogResponse {
    let launch_targets = home_targets(data_dir)
        .into_iter()
        .map(|target| (target.target.clone(), target))
        .collect::<BTreeMap<_, _>>();
    let components = load_capsule_components(data_dir);
    let active_components = crate::api::capsule_inventory::active_component_names(data_dir);
    let installed_names = installed_capsule_names(data_dir, active_components.as_ref());

    let mut capsules = crate::api::capsule_inventory::list_capsule_manifests(data_dir)
        .into_iter()
        .map(|manifest| {
            catalog_capsule_summary(manifest, &launch_targets, &components, &installed_names)
        })
        .collect::<Vec<_>>();
    capsules.sort_by(|left, right| {
        capsule_category_order(&left.category)
            .cmp(&capsule_category_order(&right.category))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut counts = CapsuleCatalogCounts {
        total: capsules.len(),
        ..Default::default()
    };
    for capsule in &capsules {
        match capsule.role.as_str() {
            "app" => counts.apps += 1,
            "viewer" => counts.viewers += 1,
            "provider" => counts.providers += 1,
            "content" => counts.content += 1,
            "shell" => counts.shell += 1,
            _ => {}
        }
        counts.interfaces += capsule.interfaces.len();
        counts.methods += capsule
            .interfaces
            .iter()
            .map(|interface| interface.methods.len())
            .sum::<usize>();
        if capsule.launchable {
            counts.launchable += 1;
        }
        if capsule.installed {
            counts.installed += 1;
        }
    }

    CapsuleCatalogResponse {
        schema: CAPSULE_CATALOG_SCHEMA.to_string(),
        counts,
        capsules,
        policy: CapsuleCatalogPolicy {
            install_state: "signed-app-install-pending".to_string(),
            install_note: "Marketplace can open installed apps now. Installing new apps will require verified app signatures, receipts, and provider policy.".to_string(),
            payment_state: "provider-rail-required".to_string(),
            payment_note: "Paid apps and services must use wallet/payment provider receipts, not embedded payment SDKs.".to_string(),
            drm_state: "provider-rail-required".to_string(),
            drm_note: "Protected apps and content must use rights, key, and decrypt providers for dDRM enforcement.".to_string(),
        },
    }
}

pub(super) fn capsule_interface_registry_summary(
    data_dir: &std::path::Path,
) -> CapsuleInterfaceRegistryResponse {
    let catalog = capsule_catalog_summary(data_dir);
    let mut interfaces = Vec::new();
    for capsule in catalog.capsules {
        for interface in capsule.interfaces {
            interfaces.push(CapsuleInterfaceSummary {
                capsule: capsule.name.clone(),
                capsule_version: capsule.version.clone(),
                title: capsule.title.clone(),
                role: capsule.role.clone(),
                capsule_type: capsule.capsule_type.clone(),
                cid: capsule.cid.clone(),
                trust_state: capsule.trust_state.clone(),
                interface,
            });
        }
    }

    let mut counts = CapsuleInterfaceRegistryCounts {
        capsules: interfaces
            .iter()
            .map(|interface| interface.capsule.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        ..Default::default()
    };
    counts.interfaces = interfaces.len();
    counts.methods = interfaces
        .iter()
        .map(|summary| summary.interface.methods.len())
        .sum();

    CapsuleInterfaceRegistryResponse {
        schema: CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
        counts,
        interfaces,
        policy: CapsuleInterfaceRegistryPolicy {
            descriptor_state: "manifest-declared".to_string(),
            descriptor_note: "Interfaces describe callable affordances declared by installed apps and providers. They are not authority grants; Runtime approval, expiry, and audit still govern invocation.".to_string(),
            invocation_state: "runtime-gated".to_string(),
            invocation_note: "0.4.0 executes low-risk Runtime Marketplace bindings and fails closed for high-risk or user-approval methods until approval/provider binding is complete.".to_string(),
        },
    }
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

fn catalog_capsule_summary(
    manifest: CapsuleManifest,
    launch_targets: &BTreeMap<String, HomeTargetSummary>,
    components: &BTreeMap<String, CapsuleComponentInfo>,
    installed_names: &BTreeSet<String>,
) -> CapsuleSummary {
    let target = launch_targets.get(&manifest.name);
    let component = components.get(&manifest.name);
    let installed = installed_names.contains(&manifest.name);
    let name = manifest.name.clone();
    let role = manifest.role.clone();
    let capsule_type = manifest.capsule_type.clone();
    let category = capsule_category(&role);
    let launchable = target.is_some() && role.is_shell_launchable();
    let signature_state = if manifest
        .signature
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        "manifest-signature-declared"
    } else {
        "no-manifest-signature"
    };
    let cid = component
        .and_then(|entry| entry.cid.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    let cid_state = if cid.is_some() {
        "cid-published"
    } else {
        "local-only"
    };

    CapsuleSummary {
        name: name.clone(),
        version: manifest.version,
        title: target
            .map(|target| target.title.clone())
            .unwrap_or_else(|| capsule_title(&name)),
        description: target
            .map(|target| target.description.clone())
            .or_else(|| manifest.description.clone())
            .unwrap_or_else(|| "Capsule metadata available through Runtime.".to_string()),
        author: manifest.author,
        role,
        capsule_type,
        category: category.to_string(),
        state: if installed { "installed" } else { "bundled" }.to_string(),
        installed,
        launchable,
        launch_target: target.map(|target| target.target.clone()),
        route: target.map(|target| target.route.clone()),
        provides: manifest.provides,
        requires: manifest
            .requires
            .into_iter()
            .map(|requirement| CapsuleRequirementSummary {
                name: requirement.name,
                kind: format!("{:?}", requirement.kind).to_ascii_lowercase(),
            })
            .collect(),
        capabilities: manifest.capabilities,
        interfaces: manifest.interfaces,
        viewer: manifest.viewer,
        cid,
        cid_state: cid_state.to_string(),
        signature_state: signature_state.to_string(),
        trust_state: capsule_trust_state(signature_state, cid_state).to_string(),
        payment_state: capsule_payment_state(&name).to_string(),
        drm_state: capsule_drm_state(&name).to_string(),
        source: if installed {
            "installed"
        } else {
            "runtime-bundle"
        }
        .to_string(),
        install_path: component.and_then(|entry| entry.install_path.clone()),
        release_path: component.and_then(|entry| entry.release_path.clone()),
        repository: component.and_then(|entry| entry.repository.clone()),
    }
}

fn capsule_category(role: &CapsuleRole) -> &'static str {
    match role {
        CapsuleRole::Shell => "Shells",
        CapsuleRole::App => "Apps",
        CapsuleRole::Viewer => "Viewers",
        CapsuleRole::Provider => "Providers",
        CapsuleRole::Content => "Content",
    }
}

fn capsule_title(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capsule_category_order(category: &str) -> u8 {
    match category {
        "Apps" => 0,
        "Viewers" => 1,
        "Content" => 2,
        "Providers" => 3,
        "Shells" => 4,
        _ => 9,
    }
}

fn capsule_trust_state(signature_state: &str, cid_state: &str) -> &'static str {
    match (signature_state, cid_state) {
        ("manifest-signature-declared", "cid-published") => "cid-with-manifest-signature",
        ("manifest-signature-declared", _) => "local-manifest-signature",
        (_, "cid-published") => "cid-without-manifest-signature",
        _ => "local-dev",
    }
}

fn capsule_payment_state(name: &str) -> &'static str {
    if name.contains("wallet") {
        "provider"
    } else {
        "not-declared"
    }
}

fn capsule_drm_state(name: &str) -> &'static str {
    if matches!(
        name,
        "drm-provider" | "rights-provider" | "key-provider" | "decrypt-provider"
    ) {
        "provider"
    } else {
        "not-declared"
    }
}

#[derive(Clone, Debug, Default)]
struct CapsuleComponentInfo {
    cid: Option<String>,
    install_path: Option<String>,
    release_path: Option<String>,
    repository: Option<String>,
}

fn load_capsule_components(data_dir: &std::path::Path) -> BTreeMap<String, CapsuleComponentInfo> {
    let Ok(bytes) = std::fs::read(data_dir.join("components.json")) else {
        return BTreeMap::new();
    };
    let Ok(manifest) = serde_json::from_slice::<crate::setup::ComponentsManifest>(&bytes) else {
        return BTreeMap::new();
    };
    let mut entries = BTreeMap::new();
    for (name, capsule) in manifest.capsules {
        entries.insert(
            name,
            CapsuleComponentInfo {
                cid: Some(capsule.cid),
                install_path: None,
                release_path: None,
                repository: capsule.repository,
            },
        );
    }
    for (name, component) in manifest.external {
        let platform = component
            .platforms
            .get("*")
            .or_else(|| component.platforms.values().next());
        entries
            .entry(name)
            .and_modify(|entry| {
                if entry.cid.as_deref().unwrap_or("").is_empty() {
                    entry.cid = platform.and_then(|platform| platform.cid.clone());
                }
                if entry.install_path.is_none() {
                    entry.install_path = component.install_path.clone();
                }
                if entry.release_path.is_none() {
                    entry.release_path =
                        platform.and_then(|platform| platform.release_path.clone());
                }
                if entry.repository.is_none() {
                    entry.repository = component.repository.clone();
                }
            })
            .or_insert_with(|| CapsuleComponentInfo {
                cid: platform.and_then(|platform| platform.cid.clone()),
                repository: component.repository,
                install_path: component.install_path,
                release_path: platform.and_then(|platform| platform.release_path.clone()),
            });
    }
    entries
}

fn installed_capsule_names(
    data_dir: &std::path::Path,
    active_components: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let root = data_dir.join("capsules");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return names;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if crate::api::capsule_inventory::installed_capsule_is_inactive(
            data_dir,
            &dir,
            name,
            active_components,
        ) {
            continue;
        }
        if crate::api::capsule_inventory::load_capsule_manifest(&dir, name).is_some() {
            names.insert(name.to_string());
        }
    }
    names
}

#[derive(Serialize)]
pub(super) struct CapsuleCatalogResponse {
    schema: String,
    counts: CapsuleCatalogCounts,
    capsules: Vec<CapsuleSummary>,
    policy: CapsuleCatalogPolicy,
}

#[derive(Default, Serialize)]
struct CapsuleCatalogCounts {
    total: usize,
    installed: usize,
    launchable: usize,
    interfaces: usize,
    methods: usize,
    apps: usize,
    viewers: usize,
    providers: usize,
    content: usize,
    shell: usize,
}

#[derive(Serialize)]
struct CapsuleCatalogPolicy {
    install_state: String,
    install_note: String,
    payment_state: String,
    payment_note: String,
    drm_state: String,
    drm_note: String,
}

#[derive(Serialize)]
struct CapsuleSummary {
    name: String,
    version: String,
    title: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    role: CapsuleRole,
    #[serde(rename = "type")]
    capsule_type: CapsuleType,
    category: String,
    state: String,
    installed: bool,
    launchable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provides: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires: Vec<CapsuleRequirementSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    interfaces: Vec<CapsuleInterfaceDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
    cid_state: String,
    signature_state: String,
    trust_state: String,
    payment_state: String,
    drm_state: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
}

#[derive(Serialize)]
struct CapsuleRequirementSummary {
    name: String,
    kind: String,
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

#[derive(Serialize)]
pub(super) struct CapsuleInterfaceRegistryResponse {
    schema: String,
    counts: CapsuleInterfaceRegistryCounts,
    interfaces: Vec<CapsuleInterfaceSummary>,
    policy: CapsuleInterfaceRegistryPolicy,
}

#[derive(Default, Serialize)]
struct CapsuleInterfaceRegistryCounts {
    capsules: usize,
    interfaces: usize,
    methods: usize,
}

#[derive(Serialize)]
struct CapsuleInterfaceSummary {
    capsule: String,
    capsule_version: String,
    title: String,
    role: CapsuleRole,
    #[serde(rename = "type")]
    capsule_type: CapsuleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
    trust_state: String,
    interface: CapsuleInterfaceDescriptor,
}

#[derive(Serialize)]
struct CapsuleInterfaceRegistryPolicy {
    descriptor_state: String,
    descriptor_note: String,
    invocation_state: String,
    invocation_note: String,
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
        write_capsule(data_dir.path(), "marketplace", "app", "wasm");
        write_capsule(data_dir.path(), "documents", "viewer", "wasm");
        write_capsule(data_dir.path(), "object-provider", "provider", "microvm");

        let catalog = capsule_catalog_summary(data_dir.path());

        assert_eq!(catalog.schema, CAPSULE_CATALOG_SCHEMA);
        assert!(catalog.counts.total >= 3);
        assert!(catalog.counts.apps >= 1);
        assert!(catalog.counts.viewers >= 1);
        assert!(catalog.counts.providers >= 1);
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert!(marketplace.launchable);
        assert_eq!(marketplace.trust_state, "local-manifest-signature");
        let provider = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "object-provider")
            .unwrap();
        assert!(!provider.launchable);
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
        // capsule_roots() also scans DEV_CAPSULES_ROOT (the repo's shipped
        // capsules/), which since the Flint merge declare their own typed
        // affordances. Global catalog/registry counts therefore reflect the whole
        // shipped surface, not just the seeded capsule — so assert on the
        // marketplace capsule's OWN contribution (what this test verifies),
        // immune to how many other capsules ship affordances.
        let catalog = capsule_catalog_summary(data_dir.path());
        let marketplace = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "marketplace")
            .unwrap();
        assert_eq!(marketplace.interfaces.len(), 1);
        assert_eq!(marketplace.interfaces[0].id, "elastos.marketplace.catalog");

        let registry = capsule_interface_registry_summary(data_dir.path());
        assert_eq!(registry.schema, CAPSULE_INTERFACE_REGISTRY_SCHEMA);
        let marketplace_entry = registry
            .interfaces
            .iter()
            .find(|entry| entry.capsule == "marketplace")
            .expect("marketplace interface listed in the registry");
        assert_eq!(marketplace_entry.interface.methods.len(), 2);
        assert_eq!(marketplace_entry.interface.methods[1].id, "capsule.open");
        assert_eq!(registry.policy.invocation_state, "runtime-gated");
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
