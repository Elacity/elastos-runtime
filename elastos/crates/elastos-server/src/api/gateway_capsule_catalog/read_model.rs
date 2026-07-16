use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleRole, CapsuleType};
use serde::Serialize;

use super::*;

pub(in crate::api::gateway) const CAPSULE_CATALOG_SCHEMA: &str = "elastos.capsules.catalog/v1";
pub(in crate::api::gateway) const CAPSULE_INTERFACE_REGISTRY_SCHEMA: &str =
    "elastos.capsules.interfaces/v1";

pub(in crate::api::gateway) fn capsule_catalog_summary(
    data_dir: &std::path::Path,
) -> CapsuleCatalogResponse {
    let launch_targets = home_launch_targets(data_dir)
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

pub(in crate::api::gateway) fn capsule_interface_registry_summary(
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

    let counts = CapsuleInterfaceRegistryCounts {
        capsules: interfaces
            .iter()
            .map(|interface| interface.capsule.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        interfaces: interfaces.len(),
        methods: interfaces
            .iter()
            .map(|summary| summary.interface.methods.len())
            .sum(),
    };

    CapsuleInterfaceRegistryResponse {
        schema: CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
        counts,
        interfaces,
        policy: CapsuleInterfaceRegistryPolicy {
            descriptor_state: "manifest-declared".to_string(),
            descriptor_note: "Interfaces describe callable affordances declared by installed apps and providers. They are not authority grants; Runtime approval, expiry, and audit still govern invocation.".to_string(),
            invocation_state: "runtime-gated".to_string(),
            invocation_note: "Runtime executes low-risk Marketplace bindings and fails closed for high-risk or user-approval methods until approval/provider binding is complete.".to_string(),
        },
    }
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

    let provides = manifest.provides;
    let capabilities = manifest.capabilities;
    let interfaces = manifest.interfaces;
    let payment_state = capsule_payment_state(&name).to_string();
    let drm_state = capsule_drm_state(&name).to_string();
    let projection = capsule_projection_summary(CapsuleProjectionInput {
        role: &role,
        launchable,
        route: target.map(|target| target.route.as_str()),
        provides: provides.as_deref(),
        capabilities: &capabilities,
        interfaces: &interfaces,
        signature_state,
        cid_state,
        payment_state: &payment_state,
        drm_state: &drm_state,
    });

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
        provides,
        requires: manifest
            .requires
            .into_iter()
            .map(|requirement| CapsuleRequirementSummary {
                name: requirement.name,
                kind: format!("{:?}", requirement.kind).to_ascii_lowercase(),
            })
            .collect(),
        capabilities,
        interfaces,
        projection,
        viewer: manifest.viewer,
        cid,
        cid_state: cid_state.to_string(),
        signature_state: signature_state.to_string(),
        trust_state: capsule_trust_state(signature_state, cid_state).to_string(),
        payment_state,
        drm_state,
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

struct CapsuleProjectionInput<'a> {
    role: &'a CapsuleRole,
    launchable: bool,
    route: Option<&'a str>,
    provides: Option<&'a str>,
    capabilities: &'a [String],
    interfaces: &'a [CapsuleInterfaceDescriptor],
    signature_state: &'a str,
    cid_state: &'a str,
    payment_state: &'a str,
    drm_state: &'a str,
}

fn capsule_projection_summary(input: CapsuleProjectionInput<'_>) -> CapsuleProjectionSummary {
    let method_count = input
        .interfaces
        .iter()
        .map(|interface| interface.methods.len())
        .sum::<usize>();
    let has_gate_metadata = method_count > 0;
    let is_provider_role = input.role == &CapsuleRole::Provider;
    let has_service_namespace = input.provides.is_some();
    let has_capability_surface = !input.capabilities.is_empty();

    CapsuleProjectionSummary {
        schema: "elastos.capsule.projection/v1".to_string(),
        web: CapsuleProjectionSurface {
            state: if input.launchable && input.route.is_some() {
                "available"
            } else if is_provider_role || has_service_namespace {
                "provider-only"
            } else {
                "not-launchable"
            }
            .to_string(),
            source: if input.route.is_some() {
                "home.summary.launch_targets"
            } else {
                "manifest.role"
            }
            .to_string(),
            route: input.route.map(ToOwned::to_owned),
            schemas: Vec::new(),
            note: Some("Web projection is launched only through Runtime Home tokens.".to_string()),
        },
        cli: CapsuleProjectionSurface {
            state: if input.launchable || method_count > 0 {
                "available"
            } else {
                "facts-only"
            }
            .to_string(),
            source: "capsules.catalog+capsules.interfaces".to_string(),
            route: None,
            schemas: vec![
                CAPSULE_CATALOG_SCHEMA.to_string(),
                CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
            ],
            note: Some("CLI renders Runtime facts and emits host intents; it does not call providers directly.".to_string()),
        },
        facts: CapsuleProjectionSurface {
            state: "available".to_string(),
            source: "runtime.catalog".to_string(),
            route: Some("/api/capsules/catalog".to_string()),
            schemas: vec![
                CAPSULE_CATALOG_SCHEMA.to_string(),
                CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
                "elastos.esp.initialize/v0".to_string(),
            ],
            note: Some("Facts are read-only projections and shells must ignore unknown fields.".to_string()),
        },
        affordances: CapsuleProjectionSurface {
            state: if method_count > 0 {
                "declared"
            } else {
                "absent"
            }
            .to_string(),
            source: "manifest.interfaces".to_string(),
            route: Some("/api/capsules/interfaces".to_string()),
            schemas: Vec::new(),
            note: Some(format!(
                "{} interfaces / {} methods; descriptors are not grants.",
                input.interfaces.len(),
                method_count
            )),
        },
        gates: CapsuleProjectionSurface {
            state: if has_gate_metadata {
                "declared"
            } else {
                "absent"
            }
            .to_string(),
            source: "manifest.interfaces.methods+routing_policy".to_string(),
            route: Some("/api/esp/initialize".to_string()),
            schemas: Vec::new(),
            note: Some("Runtime route policy, launch tokens, Inbox/Wallet approval, and provider gates remain authoritative.".to_string()),
        },
        audit_mirror: CapsuleProjectionSurface {
            state: "redacted".to_string(),
            source: "catalog.trust+system.inspector".to_string(),
            route: None,
            schemas: vec!["elastos.inspect.object/v1".to_string()],
            note: Some(format!(
                "signature={}; cid={}; payment={}; drm={}; ordinary shells receive redacted mirror facts.",
                input.signature_state, input.cid_state, input.payment_state, input.drm_state
            )),
        },
        carrier: CapsuleProjectionSurface {
            state: if has_service_namespace {
                "service-endpoint"
            } else if has_capability_surface {
                "requires-provider-intents"
            } else {
                "none"
            }
            .to_string(),
            source: "manifest.provides+manifest.capabilities".to_string(),
            route: None,
            schemas: Vec::new(),
            note: Some("Future Carrier transport must preserve the same Runtime schemas, gates, consent path, and audit.".to_string()),
        },
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
pub(in crate::api::gateway) struct CapsuleCatalogResponse {
    pub(in crate::api::gateway) schema: String,
    pub(in crate::api::gateway) counts: CapsuleCatalogCounts,
    pub(in crate::api::gateway) capsules: Vec<CapsuleSummary>,
    pub(in crate::api::gateway) policy: CapsuleCatalogPolicy,
}

#[derive(Default, Serialize)]
pub(in crate::api::gateway) struct CapsuleCatalogCounts {
    pub(in crate::api::gateway) total: usize,
    pub(in crate::api::gateway) installed: usize,
    pub(in crate::api::gateway) launchable: usize,
    pub(in crate::api::gateway) interfaces: usize,
    pub(in crate::api::gateway) methods: usize,
    pub(in crate::api::gateway) apps: usize,
    pub(in crate::api::gateway) viewers: usize,
    pub(in crate::api::gateway) providers: usize,
    pub(in crate::api::gateway) content: usize,
    pub(in crate::api::gateway) shell: usize,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleCatalogPolicy {
    pub(in crate::api::gateway) install_state: String,
    pub(in crate::api::gateway) install_note: String,
    pub(in crate::api::gateway) payment_state: String,
    pub(in crate::api::gateway) payment_note: String,
    pub(in crate::api::gateway) drm_state: String,
    pub(in crate::api::gateway) drm_note: String,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleSummary {
    pub(in crate::api::gateway) name: String,
    pub(in crate::api::gateway) version: String,
    pub(in crate::api::gateway) title: String,
    pub(in crate::api::gateway) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) author: Option<String>,
    pub(in crate::api::gateway) role: CapsuleRole,
    #[serde(rename = "type")]
    pub(in crate::api::gateway) capsule_type: CapsuleType,
    pub(in crate::api::gateway) category: String,
    pub(in crate::api::gateway) state: String,
    pub(in crate::api::gateway) installed: bool,
    pub(in crate::api::gateway) launchable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) launch_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) provides: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) requires: Vec<CapsuleRequirementSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) interfaces: Vec<CapsuleInterfaceDescriptor>,
    pub(in crate::api::gateway) projection: CapsuleProjectionSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) viewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) cid: Option<String>,
    pub(in crate::api::gateway) cid_state: String,
    pub(in crate::api::gateway) signature_state: String,
    pub(in crate::api::gateway) trust_state: String,
    pub(in crate::api::gateway) payment_state: String,
    pub(in crate::api::gateway) drm_state: String,
    pub(in crate::api::gateway) source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) install_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) release_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) repository: Option<String>,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleProjectionSummary {
    pub(in crate::api::gateway) schema: String,
    pub(in crate::api::gateway) web: CapsuleProjectionSurface,
    pub(in crate::api::gateway) cli: CapsuleProjectionSurface,
    pub(in crate::api::gateway) facts: CapsuleProjectionSurface,
    pub(in crate::api::gateway) affordances: CapsuleProjectionSurface,
    pub(in crate::api::gateway) gates: CapsuleProjectionSurface,
    pub(in crate::api::gateway) audit_mirror: CapsuleProjectionSurface,
    pub(in crate::api::gateway) carrier: CapsuleProjectionSurface,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleProjectionSurface {
    pub(in crate::api::gateway) state: String,
    pub(in crate::api::gateway) source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) route: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) schemas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) note: Option<String>,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleRequirementSummary {
    pub(in crate::api::gateway) name: String,
    pub(in crate::api::gateway) kind: String,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleInterfaceRegistryResponse {
    pub(in crate::api::gateway) schema: String,
    pub(in crate::api::gateway) counts: CapsuleInterfaceRegistryCounts,
    pub(in crate::api::gateway) interfaces: Vec<CapsuleInterfaceSummary>,
    pub(in crate::api::gateway) policy: CapsuleInterfaceRegistryPolicy,
}

#[derive(Default, Serialize)]
pub(in crate::api::gateway) struct CapsuleInterfaceRegistryCounts {
    pub(in crate::api::gateway) capsules: usize,
    pub(in crate::api::gateway) interfaces: usize,
    pub(in crate::api::gateway) methods: usize,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleInterfaceSummary {
    pub(in crate::api::gateway) capsule: String,
    pub(in crate::api::gateway) capsule_version: String,
    pub(in crate::api::gateway) title: String,
    pub(in crate::api::gateway) role: CapsuleRole,
    #[serde(rename = "type")]
    pub(in crate::api::gateway) capsule_type: CapsuleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) cid: Option<String>,
    pub(in crate::api::gateway) trust_state: String,
    pub(in crate::api::gateway) interface: CapsuleInterfaceDescriptor,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleInterfaceRegistryPolicy {
    pub(in crate::api::gateway) descriptor_state: String,
    pub(in crate::api::gateway) descriptor_note: String,
    pub(in crate::api::gateway) invocation_state: String,
    pub(in crate::api::gateway) invocation_note: String,
}
