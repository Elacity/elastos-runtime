use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{
    CapsuleExecution, CapsuleInterfaceDescriptor, CapsuleManifest, CapsuleProjection, CapsuleRole,
    CapsuleRuntimeAbi, CapsuleType,
};
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
    let accepted_content_by_viewer = accepted_content_by_viewer(data_dir);

    let mut capsules = crate::api::capsule_inventory::list_active_capsule_manifests(data_dir)
        .into_iter()
        .map(|manifest| {
            catalog_capsule_summary(
                manifest,
                &launch_targets,
                &components,
                &accepted_content_by_viewer,
            )
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
            let bindings = interface
                .methods
                .iter()
                .map(|method| static_capsule_method_binding(method).summary)
                .collect();
            interfaces.push(CapsuleInterfaceSummary {
                capsule: capsule.name.clone(),
                capsule_version: capsule.version.clone(),
                title: capsule.title.clone(),
                role: capsule.role.clone(),
                capsule_type: capsule.capsule_type.clone(),
                runtime_abi: capsule.runtime_abi.clone(),
                bus_contract: capsule.bus_contract.clone(),
                wit_world_sha256: capsule.wit_world_sha256.clone(),
                execution: capsule.execution.clone(),
                projections: capsule.projections.clone(),
                cid: capsule.cid.clone(),
                trust_state: capsule.trust_state.clone(),
                interface,
                bindings,
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
        executable_methods: interfaces
            .iter()
            .flat_map(|summary| summary.bindings.iter())
            .filter(|binding| binding.executable)
            .count(),
    };

    CapsuleInterfaceRegistryResponse {
        schema: CAPSULE_INTERFACE_REGISTRY_SCHEMA.to_string(),
        counts,
        interfaces,
        policy: CapsuleInterfaceRegistryPolicy {
            descriptor_state: "manifest-declared".to_string(),
            descriptor_note: "Interfaces describe callable affordances declared by installed apps and providers. They are not authority grants; Runtime approval, expiry, and audit still govern invocation.".to_string(),
            invocation_state: "runtime-gated".to_string(),
            invocation_note: "Runtime executes only methods marked executable by a concrete generic Runtime binding. Provider-path, approval-required, descriptive, unavailable, and unbound methods fail closed.".to_string(),
        },
    }
}

fn catalog_capsule_summary(
    manifest: CapsuleManifest,
    launch_targets: &BTreeMap<String, HomeTargetSummary>,
    components: &BTreeMap<String, CapsuleComponentInfo>,
    accepted_content_by_viewer: &BTreeMap<String, Vec<CapsuleAcceptedContentSummary>>,
) -> CapsuleSummary {
    let target = launch_targets.get(&manifest.name);
    let component = components.get(&manifest.name);
    let name = manifest.name.clone();
    let role = manifest.role.clone();
    let capsule_type = manifest.capsule_type.clone();
    let runtime_abi = manifest.runtime_abi.clone();
    let bus_contract = manifest.bus_contract.clone();
    let wit_world_sha256 = manifest.wit_world_sha256.clone();
    let execution = manifest.execution.clone();
    let declared_projections = manifest.projections.clone();
    let category = capsule_category(&role);
    // A content capsule with a bound viewer is launchable through that viewer.
    // The Runtime target, not the manifest role alone, is the launch authority.
    let launchable = target.is_some();
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
    let viewer = manifest.viewer;
    let viewer_title = viewer.as_ref().map(|viewer| {
        launch_targets
            .get(viewer)
            .map(|target| target.title.clone())
            .unwrap_or_else(|| capsule_title(viewer))
    });
    let accepted_content = accepted_content_by_viewer
        .get(&name)
        .cloned()
        .unwrap_or_default();
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
        declared_projections: &declared_projections,
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
        runtime_abi,
        bus_contract,
        wit_world_sha256,
        execution,
        projections: declared_projections,
        category: category.to_string(),
        state: "installed".to_string(),
        installed: true,
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
        viewer,
        viewer_title,
        accepted_content,
        cid,
        cid_state: cid_state.to_string(),
        signature_state: signature_state.to_string(),
        trust_state: capsule_trust_state(signature_state, cid_state).to_string(),
        payment_state,
        drm_state,
        source: "installed".to_string(),
        install_path: component.and_then(|entry| entry.install_path.clone()),
        release_path: component.and_then(|entry| entry.release_path.clone()),
        repository: component.and_then(|entry| entry.repository.clone()),
    }
}

fn accepted_content_by_viewer(
    data_dir: &std::path::Path,
) -> BTreeMap<String, Vec<CapsuleAcceptedContentSummary>> {
    let mut by_viewer: BTreeMap<String, Vec<CapsuleAcceptedContentSummary>> = BTreeMap::new();
    for capsule in crate::api::browser_capsules::list_all_viewer_bound_capsules(data_dir) {
        let title = viewer_object_shell_title(&capsule.name, capsule.description.as_deref());
        by_viewer
            .entry(capsule.viewer.clone())
            .or_default()
            .push(CapsuleAcceptedContentSummary {
                name: capsule.name,
                title,
                description: capsule.description,
                entrypoint: capsule.entrypoint,
            });
    }
    for capsules in by_viewer.values_mut() {
        capsules.sort_by(|left, right| {
            left.title
                .cmp(&right.title)
                .then_with(|| left.name.cmp(&right.name))
        });
    }
    by_viewer
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
    let service_title = match name {
        "ai-provider" => Some("AI"),
        "availability-provider" => Some("Content Availability"),
        "browser-engine-adapter" => Some("Browser Engine"),
        "chain-provider" => Some("Chains"),
        "content-block-graph-provider" => Some("Content Index"),
        "decrypt-provider" => Some("Decryption"),
        "did-provider" => Some("Identity"),
        "drm-provider" => Some("Content Protection"),
        "exit-provider" => Some("Browser Exit"),
        "ipfs-provider" => Some("Content Storage"),
        "key-provider" => Some("Key Access"),
        "llama-provider" => Some("Local AI"),
        "net-provider" => Some("Network"),
        "object-provider" => Some("Storage"),
        "operator-drive-adapter" => Some("Drive"),
        "rights-provider" => Some("Content Rights"),
        "tunnel-provider" => Some("Network Tunnel"),
        "wallet-metamask" => Some("MetaMask"),
        "wallet-provider" => Some("Wallet Security"),
        "wallet-unisat" => Some("UniSat"),
        "wallet-walletconnect" => Some("WalletConnect"),
        "webspace-provider" => Some("Webspaces"),
        _ => None,
    };
    if let Some(title) = service_title {
        return title.to_string();
    }
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
    declared_projections: &'a [CapsuleProjection],
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
    let declares_web = input.declared_projections.contains(&CapsuleProjection::Web);
    let declares_cli = input.declared_projections.contains(&CapsuleProjection::Cli);
    let declares_terminal = input
        .declared_projections
        .contains(&CapsuleProjection::Terminal);
    let declares_carrier = input
        .declared_projections
        .contains(&CapsuleProjection::Carrier);
    let has_cli_interface = input.interfaces.iter().any(|interface| {
        interface.id.contains(".terminal")
            || interface.id.ends_with(".cli")
            || interface.methods.iter().any(|method| {
                matches!(
                    method.id.as_str(),
                    "session.open" | "terminal.open" | "cli.open"
                )
            })
    });

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
            source: if declares_web && input.route.is_some() {
                "manifest.projections+home.summary.launch_targets"
            } else if input.route.is_some() {
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
            state: if declares_cli || declares_terminal || has_cli_interface {
                "available"
            } else {
                "facts-only"
            }
            .to_string(),
            source: if declares_cli || declares_terminal {
                "manifest.projections+capsules.catalog+capsules.interfaces"
            } else {
                "capsules.catalog+capsules.interfaces"
            }
            .to_string(),
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
            } else if declares_carrier || has_capability_surface {
                "requires-provider-intents"
            } else {
                "none"
            }
            .to_string(),
            source: "manifest.provides+manifest.capabilities+manifest.projections".to_string(),
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
    let current_platform = crate::setup::detect_platform();
    for (name, component) in manifest.external {
        let platform =
            crate::setup::resolve_platform_info(&component, &current_platform).or_else(|| {
                if component.platforms.len() == 1 {
                    component.platforms.values().next()
                } else {
                    None
                }
            });
        let install_path =
            crate::setup::resolve_install_path(&component, platform).map(str::to_string);
        let cid = platform.and_then(|platform| platform.cid.clone());
        let release_path = platform.and_then(|platform| platform.release_path.clone());
        let repository = component.repository.clone();
        entries
            .entry(name)
            .and_modify(|entry| {
                if entry.cid.as_deref().unwrap_or("").is_empty() {
                    entry.cid = cid.clone();
                }
                if entry.install_path.is_none() {
                    entry.install_path = install_path.clone();
                }
                if entry.release_path.is_none() {
                    entry.release_path = release_path.clone();
                }
                if entry.repository.is_none() {
                    entry.repository = repository.clone();
                }
            })
            .or_insert_with(|| CapsuleComponentInfo {
                cid,
                repository,
                install_path,
                release_path,
            });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_title_preserves_product_names() {
        assert_eq!(capsule_title("wallet-metamask"), "MetaMask");
        assert_eq!(capsule_title("wallet-unisat"), "UniSat");
        assert_eq!(capsule_title("wallet-walletconnect"), "WalletConnect");
    }

    #[test]
    fn component_metadata_uses_the_current_platform_deterministically() {
        let data_dir = tempfile::tempdir().unwrap();
        let platform = crate::setup::detect_platform();
        std::fs::write(
            data_dir.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "capsules": {},
                "external": {
                    "fixture": {
                        "platforms": {
                            platform: {
                                "release_path": "current-platform",
                                "install_path": "bin/current"
                            },
                            "other-platform-a": {
                                "release_path": "wrong-amd64",
                                "install_path": "bin/wrong-amd64"
                            },
                            "other-platform-b": {
                                "release_path": "wrong-arm64",
                                "install_path": "bin/wrong-arm64"
                            }
                        }
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let fixture = load_capsule_components(data_dir.path())
            .remove("fixture")
            .unwrap();
        assert_eq!(fixture.release_path.as_deref(), Some("current-platform"));
        assert_eq!(fixture.install_path.as_deref(), Some("bin/current"));
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) runtime_abi: Option<CapsuleRuntimeAbi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) bus_contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) wit_world_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) execution: Option<CapsuleExecution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) projections: Vec<CapsuleProjection>,
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
    pub(in crate::api::gateway) viewer_title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) accepted_content: Vec<CapsuleAcceptedContentSummary>,
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

#[derive(Clone, Serialize)]
pub(in crate::api::gateway) struct CapsuleAcceptedContentSummary {
    pub(in crate::api::gateway) name: String,
    pub(in crate::api::gateway) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) description: Option<String>,
    pub(in crate::api::gateway) entrypoint: String,
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
    pub(in crate::api::gateway) executable_methods: usize,
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
    pub(in crate::api::gateway) runtime_abi: Option<CapsuleRuntimeAbi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) bus_contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) wit_world_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) execution: Option<CapsuleExecution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::api::gateway) projections: Vec<CapsuleProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) cid: Option<String>,
    pub(in crate::api::gateway) trust_state: String,
    pub(in crate::api::gateway) interface: CapsuleInterfaceDescriptor,
    pub(in crate::api::gateway) bindings: Vec<bindings::CapsuleMethodBindingSummary>,
}

#[derive(Serialize)]
pub(in crate::api::gateway) struct CapsuleInterfaceRegistryPolicy {
    pub(in crate::api::gateway) descriptor_state: String,
    pub(in crate::api::gateway) descriptor_note: String,
    pub(in crate::api::gateway) invocation_state: String,
    pub(in crate::api::gateway) invocation_note: String,
}
