use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use elastos_common::{
    AffordanceApprovalMode, AffordanceAuditMode, AffordanceRisk, CapsuleExecution, CapsuleManifest,
    CapsuleRole, CapsuleRuntimeAbi, CapsuleType, RequirementKind,
};
use elastos_runtime::provider::{ProviderRegistration, ProviderRegistry};
use serde::Serialize;

use super::*;

const CAPSULE_CONTRACT_AUDIT_SCHEMA: &str = "elastos.capsules.contract-audit/v1";

pub(super) async fn capsule_contract_audit_summary(
    state: &GatewayState,
) -> CapsuleContractAuditResponse {
    let data_dir = &state.data_dir;
    let mut issues = Vec::new();
    let active_components = match crate::api::capsule_inventory::active_component_names(data_dir) {
        Some(names) => names,
        None => {
            issues.push(issue(
                "components_unavailable",
                None,
                "components.json is missing or invalid",
            ));
            BTreeSet::new()
        }
    };
    let source_root = crate::api::capsule_inventory::development_capsules_root();
    let installed_external_components =
        crate::api::capsule_inventory::installed_external_component_names(data_dir)
            .unwrap_or_default();
    let source_names = manifest_names(&source_root);
    let installed_root = crate::api::capsule_inventory::installed_capsules_root(data_dir);
    let installed_names = manifest_names(&installed_root);
    let expected_first_party_names = source_names
        .intersection(&active_components)
        .cloned()
        .collect::<BTreeSet<_>>();
    let installed_active_names = installed_names
        .intersection(&active_components)
        .cloned()
        .collect::<BTreeSet<_>>();
    let active_capsule_names = expected_first_party_names
        .union(&installed_active_names)
        .cloned()
        .collect::<BTreeSet<_>>();
    let catalog = capsule_catalog_summary(data_dir);
    audit_catalog_membership(
        &active_capsule_names,
        catalog.capsules.iter().map(|capsule| capsule.name.clone()),
        &mut issues,
    );

    for name in installed_names.difference(&active_capsule_names) {
        issues.push(issue(
            "inactive_product_entry",
            Some(name),
            "installed capsule is not present in the active components manifest",
        ));
    }

    let mut manifests = BTreeMap::new();
    for name in &active_capsule_names {
        let installed_path = installed_root.join(name).join("capsule.json");
        let installed_manifest = if installed_path.is_file() {
            load_manifest(&installed_path, name, &mut issues)
        } else {
            issues.push(issue(
                "active_capsule_not_installed",
                Some(name),
                "active first-party capsule has no installed manifest",
            ));
            None
        };
        let source_path = source_root.join(name).join("capsule.json");
        let source_manifest = if source_path.is_file() {
            load_manifest(&source_path, name, &mut issues)
        } else {
            None
        };
        if let (Some((_, installed_json)), Some((_, source_json))) =
            (&installed_manifest, &source_manifest)
        {
            if source_json != installed_json {
                let changed_fields = changed_top_level_fields(source_json, installed_json);
                issues.push(issue(
                    "source_installed_manifest_mismatch",
                    Some(name),
                    &format!(
                        "installed manifest differs from the first-party source manifest in: {}",
                        changed_fields.join(", ")
                    ),
                ));
            }
        }
        let installed = installed_manifest.is_some();
        if let Some((manifest, _)) = installed_manifest.or(source_manifest) {
            manifests.insert(
                name.clone(),
                ActiveCapsuleContract {
                    manifest,
                    installed,
                },
            );
        }
    }

    let launch_targets = home_launch_targets(data_dir)
        .into_iter()
        .map(|target| (target.target.clone(), target))
        .collect::<BTreeMap<_, _>>();
    let registry = state.provider_registry.as_deref();
    let registrations = match registry {
        Some(registry) => registry.registrations().await,
        None => {
            issues.push(issue(
                "provider_registry_unavailable",
                None,
                "Runtime provider registry is unavailable",
            ));
            Vec::new()
        }
    };
    let carrier_registered = match registry {
        Some(registry) => registry.carrier_invoker_registered().await,
        None => false,
    };

    let mut accepted_content: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for contract in manifests.values() {
        if let Some(viewer) = &contract.manifest.viewer {
            accepted_content
                .entry(viewer.clone())
                .or_default()
                .push(contract.manifest.name.clone());
        }
    }
    for names in accepted_content.values_mut() {
        names.sort();
    }

    let audit_context = CapsuleAuditContext {
        manifests: &manifests,
        active_components: &active_components,
        installed_external_components: &installed_external_components,
        launch_targets: &launch_targets,
        registry,
        carrier_registered,
    };
    let mut capsules = Vec::new();
    for contract in manifests.values() {
        let report = audit_manifest(
            &contract.manifest,
            contract.installed,
            &audit_context,
            accepted_content
                .get(&contract.manifest.name)
                .cloned()
                .unwrap_or_default(),
            &mut issues,
        )
        .await;
        capsules.push(report);
    }
    capsules.sort_by(|left, right| left.name.cmp(&right.name));

    let methods = capsules
        .iter()
        .flat_map(|capsule| capsule.interfaces.iter())
        .flat_map(|interface| interface.methods.iter())
        .collect::<Vec<_>>();
    let counts = CapsuleContractAuditCounts {
        active_capsules: active_capsule_names.len(),
        audited_capsules: capsules.len(),
        interfaces: capsules
            .iter()
            .map(|capsule| capsule.interfaces.len())
            .sum(),
        methods: methods.len(),
        runtime_bound_methods: methods
            .iter()
            .filter(|method| method.runtime_binding.is_some())
            .count(),
        provider_routable_methods: methods
            .iter()
            .filter(|method| method.provider_registration.is_some())
            .count(),
        unbound_executable_methods: issues
            .iter()
            .filter(|issue| issue.code == "unbound_executable_method")
            .count(),
        errors: issues.len(),
    };
    let source_capsules = source_names
        .iter()
        .map(|name| {
            let active = active_capsule_names.contains(name);
            let installed_manifest_present = installed_names.contains(name);
            let product_visible = manifests
                .get(name)
                .is_some_and(|contract| contract.installed);
            let classification = if product_visible {
                "installed-active"
            } else if active && installed_manifest_present {
                "invalid-installed-contract"
            } else if active {
                "active-source-not-installed"
            } else if installed_manifest_present {
                "installed-inactive"
            } else {
                "source-only-inactive"
            };
            CapsuleDevelopmentSourceSummary {
                name: name.clone(),
                active,
                installed_manifest_present,
                product_visible,
                classification: classification.to_string(),
            }
        })
        .collect();
    let inactive_installed_capsules = installed_names
        .difference(&active_capsule_names)
        .cloned()
        .collect();

    CapsuleContractAuditResponse {
        schema: CAPSULE_CONTRACT_AUDIT_SCHEMA.to_string(),
        ok: issues.is_empty(),
        counts,
        runtime: CapsuleRuntimeRegistrationSummary {
            carrier_registered,
            providers: registrations,
        },
        development: CapsuleDevelopmentDiagnostics {
            source_capsules,
            inactive_installed_capsules,
        },
        capsules,
        issues,
    }
}

fn audit_catalog_membership(
    active_capsules: &BTreeSet<String>,
    catalog_names: impl IntoIterator<Item = String>,
    issues: &mut Vec<CapsuleContractIssue>,
) {
    let mut occurrences = BTreeMap::<String, usize>::new();
    for name in catalog_names {
        *occurrences.entry(name).or_default() += 1;
    }
    for name in active_capsules {
        match occurrences.get(name).copied().unwrap_or_default() {
            1 => {}
            0 => issues.push(issue(
                "active_capsule_missing_from_catalog",
                Some(name),
                "active capsule is absent from the user-facing Runtime catalog",
            )),
            count => issues.push(issue(
                "duplicate_catalog_entry",
                Some(name),
                &format!("active capsule appears {count} times in the Runtime catalog"),
            )),
        }
    }
    for name in occurrences.keys() {
        if !active_capsules.contains(name) {
            issues.push(issue(
                "inactive_catalog_entry",
                Some(name),
                "Runtime catalog exposes a capsule outside the active product set",
            ));
        }
    }
}

async fn audit_manifest(
    manifest: &CapsuleManifest,
    installed: bool,
    context: &CapsuleAuditContext<'_>,
    accepted_content: Vec<String>,
    issues: &mut Vec<CapsuleContractIssue>,
) -> CapsuleContractSummary {
    let launch_target = context.launch_targets.get(&manifest.name);
    let launches_through_viewer = launch_target.is_some_and(|target| {
        manifest.role == CapsuleRole::Content
            && target.viewer.as_deref() == manifest.viewer.as_deref()
            && target.viewer.is_some()
    });
    if launch_target.is_some() && !manifest.role.is_shell_launchable() && !launches_through_viewer {
        issues.push(issue(
            "contradictory_launch_role",
            Some(&manifest.name),
            "Runtime launch target is declared for a non-launchable capsule role",
        ));
    }

    let requires = manifest
        .requires
        .iter()
        .map(|requirement| {
            let resolved = match requirement.kind {
                RequirementKind::Capsule => context
                    .manifests
                    .get(&requirement.name)
                    .is_some_and(|contract| contract.installed),
                RequirementKind::External => {
                    context.active_components.contains(&requirement.name)
                        && context
                            .installed_external_components
                            .contains(&requirement.name)
                }
            };
            if !resolved {
                issues.push(issue(
                    "unresolved_requirement",
                    Some(&manifest.name),
                    &format!("requirement {} is not active", requirement.name),
                ));
            }
            CapsuleRequirementAudit {
                name: requirement.name.clone(),
                kind: format!("{:?}", requirement.kind).to_ascii_lowercase(),
                resolved,
            }
        })
        .collect();

    if let Some(viewer) = &manifest.viewer {
        match context.manifests.get(viewer) {
            Some(viewer_contract)
                if viewer_contract.installed
                    && viewer_contract.manifest.role == CapsuleRole::Viewer =>
            {
                if !viewer_accepts_content_capsule(&viewer_contract.manifest, viewer) {
                    issues.push(issue(
                        "viewer_does_not_accept_content",
                        Some(&manifest.name),
                        &format!(
                            "viewer {viewer} does not declare content-capsule input compatibility"
                        ),
                    ));
                }
            }
            Some(viewer_contract) if !viewer_contract.installed => issues.push(issue(
                "unresolved_viewer",
                Some(&manifest.name),
                &format!("viewer {viewer} is active but not installed"),
            )),
            Some(_) => issues.push(issue(
                "viewer_role_mismatch",
                Some(&manifest.name),
                &format!("viewer {viewer} is not a viewer capsule"),
            )),
            None => issues.push(issue(
                "unresolved_viewer",
                Some(&manifest.name),
                &format!("viewer {viewer} is not active"),
            )),
        }
    }

    let provider_registration = match (&manifest.provides, context.registry) {
        (Some(provides), Some(registry)) => registry.registration_for_uri(provides).await,
        _ => None,
    };
    if manifest.role == CapsuleRole::Provider && provider_registration.is_none() {
        issues.push(issue(
            "provider_not_registered",
            Some(&manifest.name),
            "provider namespace has no live Runtime registration",
        ));
    }
    if manifest.permissions.carrier && !context.carrier_registered {
        issues.push(issue(
            "carrier_boundary_unavailable",
            Some(&manifest.name),
            "provider declares Carrier authority but Runtime has no Carrier invoker",
        ));
    }
    audit_authority_boundary(manifest, issues);

    if let (Some(authority), Some(registration)) =
        (&manifest.authority, provider_registration.as_ref())
    {
        for capability in &authority.capabilities {
            for operation in &capability.operations {
                if crate::provider_resource::provider_operation_action(
                    &registration.route,
                    operation,
                )
                .is_none()
                {
                    issues.push(issue(
                        "provider_operation_unmapped",
                        Some(&manifest.name),
                        &format!(
                            "operation {operation} has no canonical Runtime action for route {}",
                            registration.route
                        ),
                    ));
                }
            }
        }
    }

    let capabilities = audit_capabilities(manifest, context.registry).await;
    let interfaces = audit_interfaces(manifest, context.registry, issues).await;
    CapsuleContractSummary {
        name: manifest.name.clone(),
        role: manifest.role.clone(),
        capsule_type: manifest.capsule_type.clone(),
        runtime_abi: manifest.runtime_abi.clone(),
        execution: manifest.execution.clone(),
        installed,
        launch_state: if !installed {
            "not-installed"
        } else if launches_through_viewer {
            "viewer-launchable"
        } else if launch_target.is_some() {
            "launchable"
        } else if manifest.role == CapsuleRole::Provider {
            "provider-only"
        } else if manifest.role == CapsuleRole::Content {
            "content-only"
        } else {
            "installed"
        }
        .to_string(),
        launch_target: launch_target.map(|target| target.target.clone()),
        route: launch_target.map(|target| target.route.clone()),
        provides: manifest.provides.clone(),
        provider_registration,
        requires,
        capabilities,
        interfaces,
        viewer: manifest.viewer.clone(),
        accepted_content,
        accepted_inputs: accepted_inputs(manifest),
        boundary: CapsuleBoundarySummary {
            execution: execution_boundary(manifest).to_string(),
            provider: if manifest.role == CapsuleRole::Provider {
                "runtime-owned-provider-plane"
            } else {
                "runtime-mediated-only"
            }
            .to_string(),
            carrier: if manifest.permissions.carrier {
                if context.carrier_registered {
                    "runtime-carrier"
                } else {
                    "unavailable"
                }
            } else {
                "none"
            }
            .to_string(),
            direct_network: manifest.permissions.guest_network,
        },
    }
}

fn audit_authority_boundary(manifest: &CapsuleManifest, issues: &mut Vec<CapsuleContractIssue>) {
    if manifest.permissions.guest_network {
        issues.push(issue(
            "direct_network_authority",
            Some(&manifest.name),
            "product capsules must route off-box effects through Runtime providers and Carrier",
        ));
    }
}

struct ActiveCapsuleContract {
    manifest: CapsuleManifest,
    installed: bool,
}

struct CapsuleAuditContext<'a> {
    manifests: &'a BTreeMap<String, ActiveCapsuleContract>,
    active_components: &'a BTreeSet<String>,
    installed_external_components: &'a BTreeSet<String>,
    launch_targets: &'a BTreeMap<String, HomeTargetSummary>,
    registry: Option<&'a ProviderRegistry>,
    carrier_registered: bool,
}

async fn audit_capabilities(
    manifest: &CapsuleManifest,
    registry: Option<&ProviderRegistry>,
) -> Vec<CapsuleCapabilityAudit> {
    let mut capabilities = Vec::new();
    for resource in &manifest.capabilities {
        let provider_registration = match registry {
            Some(registry) => registry.registration_for_uri(resource).await,
            None => None,
        };
        capabilities.push(CapsuleCapabilityAudit {
            resource: resource.clone(),
            provider_registration,
        });
    }
    capabilities
}

async fn audit_interfaces(
    manifest: &CapsuleManifest,
    registry: Option<&ProviderRegistry>,
    issues: &mut Vec<CapsuleContractIssue>,
) -> Vec<CapsuleInterfaceAudit> {
    let mut interfaces = Vec::new();
    for interface in &manifest.interfaces {
        let mut methods = Vec::new();
        for method in &interface.methods {
            let binding = resolve_capsule_method_binding(method, registry).await;
            let runtime_binding = binding
                .runtime_binding
                .map(|binding| binding.id().to_string());
            let provider_registration = binding.provider_registration;
            let presented_executable = binding.summary.executable;
            if presented_executable && runtime_binding.is_none() {
                issues.push(issue(
                    "unbound_executable_method",
                    Some(&manifest.name),
                    &format!(
                        "{}.{} is accepted by invocation policy but has no Runtime dispatch binding",
                        interface.id, method.id
                    ),
                ));
            }
            methods.push(CapsuleMethodAudit {
                id: method.id.clone(),
                risk: method.risk.clone(),
                approval: method.approval.clone(),
                audit: method.audit.clone(),
                resource: method.resource.clone(),
                operation: method.operation.clone(),
                presented_executable,
                runtime_binding,
                provider_registration,
            });
        }
        interfaces.push(CapsuleInterfaceAudit {
            id: interface.id.clone(),
            version: interface.version.clone(),
            methods,
        });
    }
    interfaces
}

fn manifest_names(root: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            path.join("capsule.json")
                .is_file()
                .then(|| entry.file_name().to_string_lossy().to_string())
        })
        .collect()
}

fn accepted_inputs(manifest: &CapsuleManifest) -> Vec<serde_json::Value> {
    let mut accepted = manifest
        .interfaces
        .iter()
        .flat_map(|interface| interface.methods.iter())
        .filter_map(|method| method.input_schema.as_ref())
        .filter_map(|schema| schema.get("accepts"))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    accepted.sort_by_key(serde_json::Value::to_string);
    accepted.dedup();
    accepted
}

fn viewer_accepts_content_capsule(manifest: &CapsuleManifest, viewer: &str) -> bool {
    accepted_inputs(manifest).iter().any(|accepted| {
        accepted.get("kind").and_then(serde_json::Value::as_str) == Some("content_capsule")
            && accepted
                .get("viewer")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|declared| declared == viewer)
    })
}

fn load_manifest(
    path: &Path,
    expected_name: &str,
    issues: &mut Vec<CapsuleContractIssue>,
) -> Option<(CapsuleManifest, serde_json::Value)> {
    let value = match read_json(path) {
        Ok(value) => value,
        Err(error) => {
            issues.push(issue("invalid_manifest", Some(expected_name), &error));
            return None;
        }
    };
    let manifest = match serde_json::from_value::<CapsuleManifest>(value.clone()) {
        Ok(manifest) => manifest,
        Err(error) => {
            issues.push(issue(
                "invalid_manifest",
                Some(expected_name),
                &format!("{}: {error}", path.display()),
            ));
            return None;
        }
    };
    if manifest.name != expected_name {
        issues.push(issue(
            "contradictory_manifest_name",
            Some(expected_name),
            &format!("manifest name is {}", manifest.name),
        ));
        return None;
    }
    if let Err(error) = manifest.validate() {
        issues.push(issue(
            "invalid_manifest",
            Some(expected_name),
            &format!("{}: {error}", path.display()),
        ));
        return None;
    }
    Some((manifest, value))
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn changed_top_level_fields(
    source: &serde_json::Value,
    installed: &serde_json::Value,
) -> Vec<String> {
    let source = source.as_object();
    let installed = installed.as_object();
    let mut keys = BTreeSet::new();
    if let Some(source) = source {
        keys.extend(source.keys().cloned());
    }
    if let Some(installed) = installed {
        keys.extend(installed.keys().cloned());
    }
    keys.into_iter()
        .filter(|key| {
            source.and_then(|value| value.get(key)) != installed.and_then(|value| value.get(key))
        })
        .collect()
}

fn execution_boundary(manifest: &CapsuleManifest) -> &'static str {
    match manifest.execution {
        Some(CapsuleExecution::Component) => "component",
        Some(CapsuleExecution::WebProjection) => "runtime-projection",
        Some(CapsuleExecution::Microvm) => "runtime-supervised-microvm",
        Some(CapsuleExecution::Data) | None if manifest.role == CapsuleRole::Content => {
            "inert-content"
        }
        _ if manifest.capsule_type == CapsuleType::MicroVM => "runtime-supervised-microvm",
        _ => "runtime-supervised",
    }
}

fn issue(code: &str, capsule: Option<&str>, detail: &str) -> CapsuleContractIssue {
    CapsuleContractIssue {
        severity: "error".to_string(),
        code: code.to_string(),
        capsule: capsule.map(ToOwned::to_owned),
        detail: detail.to_string(),
    }
}

#[derive(Serialize)]
pub(super) struct CapsuleContractAuditResponse {
    pub(super) schema: String,
    pub(super) ok: bool,
    pub(super) counts: CapsuleContractAuditCounts,
    pub(super) runtime: CapsuleRuntimeRegistrationSummary,
    pub(super) development: CapsuleDevelopmentDiagnostics,
    pub(super) capsules: Vec<CapsuleContractSummary>,
    pub(super) issues: Vec<CapsuleContractIssue>,
}

#[derive(Serialize)]
pub(super) struct CapsuleDevelopmentDiagnostics {
    source_capsules: Vec<CapsuleDevelopmentSourceSummary>,
    inactive_installed_capsules: Vec<String>,
}

#[derive(Serialize)]
struct CapsuleDevelopmentSourceSummary {
    name: String,
    active: bool,
    installed_manifest_present: bool,
    product_visible: bool,
    classification: String,
}

#[derive(Serialize)]
pub(super) struct CapsuleContractAuditCounts {
    active_capsules: usize,
    audited_capsules: usize,
    interfaces: usize,
    methods: usize,
    runtime_bound_methods: usize,
    provider_routable_methods: usize,
    unbound_executable_methods: usize,
    errors: usize,
}

#[derive(Serialize)]
pub(super) struct CapsuleRuntimeRegistrationSummary {
    carrier_registered: bool,
    providers: Vec<ProviderRegistration>,
}

#[derive(Serialize)]
pub(super) struct CapsuleContractSummary {
    name: String,
    role: CapsuleRole,
    #[serde(rename = "type")]
    capsule_type: CapsuleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_abi: Option<CapsuleRuntimeAbi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<CapsuleExecution>,
    installed: bool,
    launch_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provides: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_registration: Option<ProviderRegistration>,
    requires: Vec<CapsuleRequirementAudit>,
    capabilities: Vec<CapsuleCapabilityAudit>,
    interfaces: Vec<CapsuleInterfaceAudit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    accepted_content: Vec<String>,
    accepted_inputs: Vec<serde_json::Value>,
    boundary: CapsuleBoundarySummary,
}

#[derive(Serialize)]
struct CapsuleRequirementAudit {
    name: String,
    kind: String,
    resolved: bool,
}

#[derive(Serialize)]
struct CapsuleCapabilityAudit {
    resource: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_registration: Option<ProviderRegistration>,
}

#[derive(Serialize)]
struct CapsuleInterfaceAudit {
    id: String,
    version: String,
    methods: Vec<CapsuleMethodAudit>,
}

#[derive(Serialize)]
struct CapsuleMethodAudit {
    id: String,
    risk: AffordanceRisk,
    approval: AffordanceApprovalMode,
    audit: AffordanceAuditMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    presented_executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_binding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_registration: Option<ProviderRegistration>,
}

#[derive(Serialize)]
struct CapsuleBoundarySummary {
    execution: String,
    provider: String,
    carrier: String,
    direct_network: bool,
}

#[derive(Serialize)]
pub(super) struct CapsuleContractIssue {
    severity: String,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    capsule: Option<String>,
    detail: String,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, OnceLock};

    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};

    use super::*;

    struct FixtureProvider;

    #[async_trait::async_trait]
    impl Provider for FixtureProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Ok(ResourceResponse::Ok)
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["ipfs"]
        }

        fn name(&self) -> &'static str {
            "fixture-ipfs-provider"
        }
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    fn source_manifest(name: &str) -> PathBuf {
        repo_root().join("capsules").join(name).join("capsule.json")
    }

    fn install_manifest(data_dir: &Path, name: &str) {
        fn copy_tree(source: &Path, target: &Path) {
            if source.is_dir() {
                std::fs::create_dir_all(target).unwrap();
                for entry in std::fs::read_dir(source).unwrap().flatten() {
                    copy_tree(&entry.path(), &target.join(entry.file_name()));
                }
            } else {
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(source, target).unwrap();
            }
        }

        let source = source_manifest(name).parent().unwrap().to_path_buf();
        let target = data_dir.join("capsules").join(name);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::copy(source.join("capsule.json"), target.join("capsule.json")).unwrap();
        if source.join("browser").is_dir() {
            copy_tree(&source.join("browser"), &target.join("browser"));
        }
        let manifest: CapsuleManifest =
            serde_json::from_slice(&std::fs::read(source.join("capsule.json")).unwrap()).unwrap();
        if source.join(&manifest.entrypoint).is_file() {
            copy_tree(
                &source.join(&manifest.entrypoint),
                &target.join(&manifest.entrypoint),
            );
        }
    }

    fn write_components(data_dir: &Path, names: &[&str]) {
        let external = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    serde_json::json!({
                        "install_path": format!("bin/{name}"),
                        "platforms": {}
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let capsules = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    serde_json::json!({
                        "cid": "",
                        "sha256": "",
                        "size": 0
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        std::fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": external,
                "capsules": capsules,
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(data_dir.join("bin")).unwrap();
        for name in names {
            std::fs::write(data_dir.join("bin").join(name), b"fixture").unwrap();
        }
    }

    fn gateway_state(data_dir: &Path, registry: Arc<ProviderRegistry>) -> GatewayState {
        GatewayState {
            provider_registry: Some(registry),
            identity_manager: Arc::new(OnceLock::new()),
            cache_dir: data_dir.join("cache"),
            data_dir: data_dir.to_path_buf(),
            audit_log: Arc::new(std::sync::OnceLock::new()),
        }
    }

    fn issue_codes(report: &CapsuleContractAuditResponse) -> BTreeSet<&str> {
        report
            .issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect()
    }

    #[test]
    fn audit_rejects_direct_capsule_network_authority() {
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source_manifest("home-cli")).unwrap()).unwrap();
        raw["permissions"]["guest_network"] = serde_json::json!(true);
        let manifest: CapsuleManifest = serde_json::from_value(raw).unwrap();
        let mut issues = Vec::new();

        audit_authority_boundary(&manifest, &mut issues);

        assert!(issues
            .iter()
            .any(|issue| issue.code == "direct_network_authority"));
    }

    #[test]
    fn catalog_membership_requires_one_entry_per_active_capsule() {
        let active = BTreeSet::from(["home".to_string(), "system".to_string()]);
        let mut issues = Vec::new();

        audit_catalog_membership(
            &active,
            [
                "home".to_string(),
                "home".to_string(),
                "retired".to_string(),
            ],
            &mut issues,
        );

        let codes = issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("duplicate_catalog_entry"));
        assert!(codes.contains("active_capsule_missing_from_catalog"));
        assert!(codes.contains("inactive_catalog_entry"));
    }

    #[tokio::test]
    async fn audit_covers_the_derived_active_first_party_set() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["home-cli", "gba-emulator", "gba-ucity"]);
        let first_party = BTreeSet::from([
            "gba-emulator".to_string(),
            "gba-ucity".to_string(),
            "home-cli".to_string(),
        ]);
        for name in &first_party {
            install_manifest(data_dir.path(), name);
        }
        let active = crate::api::capsule_inventory::active_capsule_names(data_dir.path()).unwrap();
        assert_eq!(active, first_party);

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        let reported = report
            .capsules
            .iter()
            .map(|capsule| capsule.name.clone())
            .collect::<BTreeSet<_>>();

        assert_eq!(report.counts.active_capsules, first_party.len());
        assert_eq!(report.counts.audited_capsules, first_party.len());
        assert_eq!(reported, first_party);
        assert!(!reported.contains("browser"));
        let source_only = report
            .development
            .source_capsules
            .iter()
            .find(|capsule| capsule.name == "browser")
            .expect("source-only capsule must remain available to explicit diagnostics");
        assert!(!source_only.product_visible);
        assert_eq!(source_only.classification, "source-only-inactive");
        let installed_gba = report
            .development
            .source_capsules
            .iter()
            .find(|capsule| capsule.name == "gba-emulator")
            .unwrap();
        assert!(installed_gba.product_visible);
        assert_eq!(installed_gba.classification, "installed-active");
        let gba = report
            .capsules
            .iter()
            .find(|capsule| capsule.name == "gba-emulator")
            .unwrap();
        assert_eq!(gba.accepted_content, vec!["gba-ucity"]);
        assert!(gba
            .accepted_inputs
            .iter()
            .any(|accepted| { accepted.get("extensions") == Some(&serde_json::json!([".gba"])) }));
        assert!(!report.issues.iter().any(|issue| {
            issue.code == "viewer_does_not_accept_content"
                && issue.capsule.as_deref() == Some("gba-ucity")
        }));
        let ucity = report
            .capsules
            .iter()
            .find(|capsule| capsule.name == "gba-ucity")
            .unwrap();
        assert_eq!(ucity.launch_state, "viewer-launchable");
        assert!(!report.issues.iter().any(|issue| {
            issue.code == "contradictory_launch_role"
                && issue.capsule.as_deref() == Some("gba-ucity")
        }));
    }

    #[tokio::test]
    async fn audit_fails_inactive_entries_without_presenting_unbound_methods_as_executable() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["home-cli"]);
        install_manifest(data_dir.path(), "home-cli");
        install_manifest(data_dir.path(), "wallet-metamask");

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        let codes = issue_codes(&report);

        assert!(!report.ok);
        assert!(codes.contains("inactive_product_entry"));
        assert!(!codes.contains("unbound_executable_method"));
        assert_eq!(report.counts.unbound_executable_methods, 0);
        assert!(report.counts.runtime_bound_methods > 0);
        assert!(report
            .development
            .inactive_installed_capsules
            .contains(&"wallet-metamask".to_string()));
    }

    #[tokio::test]
    async fn audit_keeps_an_active_missing_install_in_the_report() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["home-cli"]);

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        let home_cli = report
            .capsules
            .iter()
            .find(|capsule| capsule.name == "home-cli")
            .expect("active source capsule must remain visible in the report");

        assert!(!report.ok);
        assert!(!home_cli.installed);
        assert_eq!(home_cli.launch_state, "not-installed");
        assert!(issue_codes(&report).contains("active_capsule_not_installed"));
        assert_eq!(report.counts.active_capsules, 1);
        assert_eq!(report.counts.audited_capsules, 1);
    }

    #[tokio::test]
    async fn audit_fails_unresolved_capsule_requirements() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["archive-manager"]);
        install_manifest(data_dir.path(), "archive-manager");

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;

        assert!(issue_codes(&report).contains("unresolved_requirement"));
    }

    #[tokio::test]
    async fn audit_resolves_external_requirements_only_from_installed_artifacts() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["ipfs-provider"]);
        let components_path = data_dir.path().join("components.json");
        let mut components: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&components_path).unwrap()).unwrap();
        components["external"]["kubo"] = serde_json::json!({
            "install_path": "bin/kubo",
            "platforms": {}
        });
        std::fs::write(
            &components_path,
            serde_json::to_vec_pretty(&components).unwrap(),
        )
        .unwrap();
        install_manifest(data_dir.path(), "ipfs-provider");

        let missing = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        assert!(missing.issues.iter().any(|issue| {
            issue.code == "unresolved_requirement"
                && issue.capsule.as_deref() == Some("ipfs-provider")
        }));

        std::fs::create_dir_all(data_dir.path().join("bin")).unwrap();
        std::fs::write(data_dir.path().join("bin/kubo"), b"fixture").unwrap();
        let installed = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        assert!(!installed.issues.iter().any(|issue| {
            issue.code == "unresolved_requirement"
                && issue.capsule.as_deref() == Some("ipfs-provider")
        }));
    }

    #[tokio::test]
    async fn audit_fails_contradictory_installed_manifests() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["home-cli"]);
        install_manifest(data_dir.path(), "home-cli");
        let path = data_dir.path().join("capsules/home-cli/capsule.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["role"] = serde_json::json!("content");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;

        assert!(issue_codes(&report).contains("invalid_manifest"));
        assert_eq!(report.counts.audited_capsules, 1);
        assert!(!report.capsules[0].installed);
        assert_eq!(report.capsules[0].launch_state, "not-installed");
    }

    #[tokio::test]
    async fn audit_names_changed_source_contract_fields() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["home-cli"]);
        install_manifest(data_dir.path(), "home-cli");
        let path = data_dir.path().join("capsules/home-cli/capsule.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["description"] = serde_json::json!("Installed description drift");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        let report = capsule_contract_audit_summary(&gateway_state(
            data_dir.path(),
            Arc::new(ProviderRegistry::new()),
        ))
        .await;
        let mismatch = report
            .issues
            .iter()
            .find(|issue| issue.code == "source_installed_manifest_mismatch")
            .expect("valid contract drift must fail the audit");

        assert!(mismatch.detail.ends_with("description"));
    }

    #[tokio::test]
    async fn audit_uses_the_live_provider_registration() {
        let data_dir = tempfile::tempdir().unwrap();
        write_components(data_dir.path(), &["ipfs-provider"]);
        install_manifest(data_dir.path(), "ipfs-provider");
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("ipfs", Arc::new(FixtureProvider))
            .await
            .unwrap();

        let report =
            capsule_contract_audit_summary(&gateway_state(data_dir.path(), registry)).await;
        let ipfs = report
            .capsules
            .iter()
            .find(|capsule| capsule.name == "ipfs-provider")
            .unwrap();

        assert_eq!(
            ipfs.provider_registration
                .as_ref()
                .map(|item| item.route.as_str()),
            Some("ipfs")
        );
        assert!(!report.issues.iter().any(|issue| {
            issue.code == "provider_not_registered"
                && issue.capsule.as_deref() == Some("ipfs-provider")
        }));
    }
}
