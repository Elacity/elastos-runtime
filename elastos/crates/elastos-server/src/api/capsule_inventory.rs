use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use elastos_common::CapsuleManifest;

const DEV_CAPSULES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");

pub(crate) fn capsule_dir_candidates(data_dir: &Path, app: &str) -> [PathBuf; 2] {
    [
        data_dir.join("capsules").join(app),
        PathBuf::from(DEV_CAPSULES_ROOT).join(app),
    ]
}

pub(crate) fn capsule_roots(data_dir: &Path) -> [PathBuf; 2] {
    [data_dir.join("capsules"), PathBuf::from(DEV_CAPSULES_ROOT)]
}

pub(crate) fn active_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let bytes = std::fs::read(data_dir.join("components.json")).ok()?;
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(&bytes).ok()?;
    let mut names: BTreeSet<String> = manifest.external.keys().cloned().collect();
    names.extend(manifest.capsules.keys().cloned());
    Some(names)
}

pub(crate) fn installed_capsule_is_inactive(
    data_dir: &Path,
    dir: &Path,
    name: &str,
    active_components: Option<&BTreeSet<String>>,
) -> bool {
    dir == data_dir.join("capsules").join(name)
        && active_components.is_some_and(|components| !components.contains(name))
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

pub(crate) fn list_capsule_manifests(data_dir: &Path) -> Vec<CapsuleManifest> {
    let mut capsules = BTreeMap::new();
    let active_components = active_component_names(data_dir);
    for root in capsule_roots(data_dir) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if installed_capsule_is_inactive(data_dir, &dir, name, active_components.as_ref()) {
                continue;
            }
            let Some(manifest) = load_capsule_manifest(&dir, name) else {
                continue;
            };
            capsules.entry(manifest.name.clone()).or_insert(manifest);
        }
    }

    capsules.into_values().collect()
}
