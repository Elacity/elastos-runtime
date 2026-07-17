use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use elastos_common::CapsuleManifest;

const DEV_CAPSULES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");

pub(crate) fn installed_capsules_root(data_dir: &Path) -> PathBuf {
    data_dir.join("capsules")
}

pub(crate) fn development_capsules_root() -> PathBuf {
    PathBuf::from(DEV_CAPSULES_ROOT)
}

fn components_manifest(data_dir: &Path) -> Option<crate::setup::ComponentsManifest> {
    let bytes = std::fs::read(data_dir.join("components.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn active_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(installed_external_component_names_from_manifest(
        data_dir, &manifest,
    ))
}

pub(crate) fn active_capsule_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(
        installed_external_component_names_from_manifest(data_dir, &manifest)
            .into_iter()
            .filter(|name| {
                load_capsule_manifest(&installed_capsules_root(data_dir).join(name), name).is_some()
            })
            .collect(),
    )
}

pub(crate) fn installed_external_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(installed_external_component_names_from_manifest(
        data_dir, &manifest,
    ))
}

fn installed_external_component_names_from_manifest(
    data_dir: &Path,
    manifest: &crate::setup::ComponentsManifest,
) -> BTreeSet<String> {
    let platform = crate::setup::detect_platform();
    manifest
        .external
        .iter()
        .filter_map(|(name, component)| {
            let platform_info = crate::setup::resolve_platform_info(component, &platform);
            let install_path = crate::setup::resolve_install_path(component, platform_info)?;
            let install_path = Path::new(install_path);
            if install_path.is_absolute()
                || install_path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                || !data_dir.join(install_path).exists()
            {
                return None;
            }
            Some(name.clone())
        })
        .collect()
}

pub(crate) fn installed_active_capsule_dir(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let active_capsules = active_capsule_names(data_dir)?;
    if !active_capsules.contains(name) {
        return None;
    }
    let dir = installed_capsules_root(data_dir).join(name);
    load_capsule_manifest(&dir, name).map(|_| dir)
}

pub(crate) fn load_capsule_manifest(dir: &Path, expected_name: &str) -> Option<CapsuleManifest> {
    if !dir.is_dir() {
        return None;
    }

    let manifest_path = dir.join("capsule.json");
    let bytes = std::fs::read(&manifest_path).ok()?;
    let manifest: CapsuleManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.validate().is_err() || manifest.name != expected_name {
        return None;
    }
    Some(manifest)
}

fn list_manifests(root: &Path, allowed_names: Option<&BTreeSet<String>>) -> Vec<CapsuleManifest> {
    let mut capsules = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if allowed_names.is_some_and(|allowed| !allowed.contains(name)) {
            continue;
        }
        let Some(manifest) = load_capsule_manifest(&dir, name) else {
            continue;
        };
        capsules.insert(manifest.name.clone(), manifest);
    }
    capsules.into_values().collect()
}

pub(crate) fn list_active_capsule_manifests(data_dir: &Path) -> Vec<CapsuleManifest> {
    let Some(active_capsules) = active_capsule_names(data_dir) else {
        return Vec::new();
    };
    list_manifests(&installed_capsules_root(data_dir), Some(&active_capsules))
}

#[cfg(test)]
pub(crate) fn list_development_capsule_manifests() -> Vec<CapsuleManifest> {
    list_manifests(&development_capsules_root(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_inventory_excludes_source_only_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            data_dir.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "gba-emulator": {
                        "install_path": "capsules/gba-emulator",
                        "platforms": {}
                    },
                    "gba-ucity": {
                        "install_path": "capsules/gba-ucity",
                        "platforms": {}
                    },
                    "binary-only": {
                        "install_path": "bin/binary-only",
                        "platforms": {}
                    }
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let binary_capsule = data_dir.path().join("capsules/binary-only");
        std::fs::create_dir_all(&binary_capsule).unwrap();
        std::fs::write(
            binary_capsule.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "binary-only",
                "version": "0.1.0",
                "role": "app",
                "type": "data",
                "entrypoint": "index.html"
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(binary_capsule.join("index.html"), "test").unwrap();

        let names = list_active_capsule_manifests(data_dir.path())
            .into_iter()
            .map(|manifest| manifest.name)
            .collect::<BTreeSet<_>>();

        assert!(
            names.is_empty(),
            "source directories are not product installs"
        );
        let development_names = list_development_capsule_manifests()
            .into_iter()
            .map(|manifest| manifest.name)
            .collect::<BTreeSet<_>>();
        assert!(development_names.contains("gba-emulator"));
        assert!(development_names.contains("gba-ucity"));
    }

    #[test]
    fn product_inventory_requires_an_installed_component_and_capsule_contract() {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            data_dir.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "object-provider": {
                        "install_path": "bin/object-provider",
                        "platforms": {}
                    },
                    "missing-provider": {
                        "install_path": "bin/missing-provider",
                        "platforms": {}
                    }
                },
                "capsules": {
                    "source-registry-only": {
                        "cid": "bafy-source",
                        "sha256": "",
                        "size": 0
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(data_dir.path().join("bin")).unwrap();
        std::fs::write(data_dir.path().join("bin/object-provider"), "provider").unwrap();
        let contract_dir = data_dir.path().join("capsules/object-provider");
        std::fs::create_dir_all(&contract_dir).unwrap();
        std::fs::copy(
            development_capsules_root().join("object-provider/capsule.json"),
            contract_dir.join("capsule.json"),
        )
        .unwrap();

        assert_eq!(
            active_component_names(data_dir.path()).unwrap(),
            BTreeSet::from(["object-provider".to_string()])
        );
        assert_eq!(
            active_capsule_names(data_dir.path()).unwrap(),
            BTreeSet::from(["object-provider".to_string()])
        );
    }
}
