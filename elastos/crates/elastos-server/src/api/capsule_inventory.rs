use std::collections::BTreeSet;
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
