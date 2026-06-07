use std::collections::{BTreeMap, BTreeSet};

use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType};
use serde::Serialize;

use super::*;

const CAPSULE_CATALOG_SCHEMA: &str = "elastos.capsules.catalog/v1";
const FIRST_PARTY_SOURCE_REPOSITORY: &str = "https://github.com/Elacity/elastos-runtime";

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

    let mut counts = CapsuleCatalogCounts::default();
    counts.total = capsules.len();
    for capsule in &capsules {
        match capsule.role.as_str() {
            "app" => counts.apps += 1,
            "viewer" => counts.viewers += 1,
            "provider" => counts.providers += 1,
            "content" => counts.content += 1,
            "shell" => counts.shell += 1,
            _ => {}
        }
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
            install_state: "signed-cid-install-pending".to_string(),
            install_note: "Marketplace can launch installed capsules now. Remote install must verify signed CID manifests and provider policy before enabling one-click install.".to_string(),
            payment_state: "provider-rail-required".to_string(),
            payment_note: "Paid capsules and services must use wallet/payment provider receipts, not embedded payment SDKs.".to_string(),
            drm_state: "provider-rail-required".to_string(),
            drm_note: "Protected capsules and content must use rights, key, and decrypt providers for dDRM enforcement.".to_string(),
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
        "signed"
    } else {
        "unsigned-local"
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
        repository: component
            .and_then(|entry| entry.repository.clone())
            .or_else(|| Some(FIRST_PARTY_SOURCE_REPOSITORY.to_string())),
        source_path: component
            .and_then(|entry| entry.source_path.clone())
            .or_else(|| Some(format!("capsules/{name}"))),
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
        ("signed", "cid-published") => "signed-cid",
        ("signed", _) => "signed-local",
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
    source_path: Option<String>,
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
                source_path: capsule.source_path,
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
                if entry.source_path.is_none() {
                    entry.source_path = component
                        .source_path
                        .clone()
                        .or_else(|| component.install_path.clone());
                }
            })
            .or_insert_with(|| CapsuleComponentInfo {
                cid: platform.and_then(|platform| platform.cid.clone()),
                source_path: component
                    .source_path
                    .clone()
                    .or_else(|| component.install_path.clone()),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
}

#[derive(Serialize)]
struct CapsuleRequirementSummary {
    name: String,
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_capsule(data_dir: &std::path::Path, name: &str, role: &str, capsule_type: &str) {
        let dir = data_dir.join("capsules").join(name);
        fs::create_dir_all(&dir).unwrap();
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
                "entrypoint": "index.html",
                "signature": "test-signature"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    }

    #[test]
    fn capsule_catalog_lists_roles_and_launchable_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        write_capsule(data_dir.path(), "marketplace", "app", "data");
        write_capsule(data_dir.path(), "documents", "viewer", "data");
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
        assert_eq!(marketplace.trust_state, "signed-local");
        let provider = catalog
            .capsules
            .iter()
            .find(|capsule| capsule.name == "object-provider")
            .unwrap();
        assert!(!provider.launchable);
        assert_eq!(
            provider.repository.as_deref(),
            Some(FIRST_PARTY_SOURCE_REPOSITORY)
        );
        assert_eq!(
            provider.source_path.as_deref(),
            Some("capsules/object-provider")
        );
    }
}
