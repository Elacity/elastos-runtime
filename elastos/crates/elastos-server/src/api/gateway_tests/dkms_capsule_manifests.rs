//! DKMS -> ESP manifest migration for the four dKMS product capsules.
//!
//! `creator`, `elacity-player`, `ddrm-viewer`, and `content-market` were ported
//! in with the OLD dkms manifest convention. Runtime's `CapsuleManifest::validate`
//! (the same validation `capsule_inventory::load_capsule_manifest` runs before a
//! capsule is allowed into the product catalog behind `/api/capsules/catalog`)
//! only accepts the current `elastos.capsule/v1` shape. A manifest that fails
//! `validate()` is silently dropped by `list_active_capsule_manifests` -- there is
//! no "invalid" flag surfaced in the catalog response, so presence in the loaded
//! set IS the validity signal.

use std::path::Path;

use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType};

use crate::api::gateway::capsule_catalog_summary;

const DKMS_CAPSULES: [&str; 4] = ["creator", "elacity-player", "ddrm-viewer", "content-market"];

fn source_capsule_dir(name: &str) -> std::path::PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules")).join(name)
}

/// Mirrors the selective install used elsewhere in this crate's capsule tests:
/// copy only the manifest and (if present as a real file) its entrypoint, never
/// the whole source tree -- `content-market` carries a `target/` build directory
/// that must not be copied into a tempdir on every test run.
fn install_dkms_capsule(data_dir: &Path, name: &str) {
    let source = source_capsule_dir(name);
    let target = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&target).unwrap();
    std::fs::copy(source.join("capsule.json"), target.join("capsule.json")).unwrap();

    let manifest: CapsuleManifest =
        serde_json::from_slice(&std::fs::read(source.join("capsule.json")).unwrap())
            .unwrap_or_else(|e| panic!("{name} capsule.json must parse as JSON: {e}"));
    let entrypoint_source = source.join(&manifest.entrypoint);
    if entrypoint_source.is_file() {
        let entrypoint_target = target.join(&manifest.entrypoint);
        std::fs::create_dir_all(entrypoint_target.parent().unwrap()).unwrap();
        std::fs::copy(&entrypoint_source, &entrypoint_target).unwrap();
    }
}

fn write_dkms_components(data_dir: &Path) {
    let external = DKMS_CAPSULES
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                serde_json::json!({
                    "install_path": format!("capsules/{name}"),
                    "platforms": {}
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    std::fs::write(
        data_dir.join("components.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "external": external,
            "capsules": {},
            "profiles": {}
        }))
        .unwrap(),
    )
    .unwrap();
}

fn install_all_dkms_capsules(data_dir: &Path) {
    write_dkms_components(data_dir);
    for name in DKMS_CAPSULES {
        install_dkms_capsule(data_dir, name);
    }
}

/// Every dkms capsule.json must parse as JSON and pass `CapsuleManifest::validate`
/// against the shipped ESP schema on its own, independent of catalog wiring.
#[test]
fn dkms_capsule_manifests_parse_and_validate_standalone() {
    for name in DKMS_CAPSULES {
        let raw = std::fs::read(source_capsule_dir(name).join("capsule.json"))
            .unwrap_or_else(|e| panic!("{name}/capsule.json must be readable: {e}"));
        let manifest: CapsuleManifest = serde_json::from_slice(&raw)
            .unwrap_or_else(|e| panic!("{name}/capsule.json must parse: {e}"));
        manifest
            .validate()
            .unwrap_or_else(|e| panic!("{name}/capsule.json failed ESP validation: {e}"));
        assert_eq!(
            manifest.name, name,
            "{name} manifest name must match its directory"
        );
    }
}

/// content-market is the one provider/microvm capsule of the four; its
/// authority-declared ops must stay `elastos://market/*` so the G3b carrier
/// conformance pin (preview action == enforced action) keeps covering it.
#[test]
fn content_market_keeps_its_provider_authority_shape() {
    let raw = std::fs::read(source_capsule_dir("content-market").join("capsule.json")).unwrap();
    let manifest: CapsuleManifest = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest.role, CapsuleRole::Provider);
    assert_eq!(manifest.capsule_type, CapsuleType::MicroVM);
    assert_eq!(manifest.provides.as_deref(), Some("elastos://market/*"));
    let authority = manifest
        .authority
        .expect("content-market must keep declaring provider authority");
    let mut ops: Vec<&str> = authority
        .capabilities
        .iter()
        .flat_map(|cap| cap.operations.iter().map(String::as_str))
        .collect();
    ops.sort_unstable();
    assert_eq!(ops, ["reconstruct_listing", "status"]);
}

/// End-to-end: install all four capsules the way Runtime does (active in
/// components.json, materialized under `<data_dir>/capsules/<name>`) and confirm
/// the exact function backing `/api/capsules/catalog`
/// (`capsule_inventory::list_active_capsule_manifests`) loads all four. A
/// manifest that fails `validate()` never reaches this list -- there is no
/// separate "invalid" flag to check.
#[test]
fn dkms_capsules_load_into_the_active_catalog_inventory() {
    let data_dir = tempfile::tempdir().unwrap();
    install_all_dkms_capsules(data_dir.path());

    let loaded = crate::api::capsule_inventory::list_active_capsule_manifests(data_dir.path());
    let loaded_names: std::collections::BTreeSet<&str> = loaded
        .iter()
        .map(|manifest| manifest.name.as_str())
        .collect();

    for name in DKMS_CAPSULES {
        assert!(
            loaded_names.contains(name),
            "{name} failed manifest validation and was dropped from the active catalog inventory; loaded={loaded_names:?}"
        );
    }
}

/// Shell-launchability pin: `list_launchable_browser_capsules` (the function
/// behind the Home launcher) silently drops any capsule that
/// `load_browser_capsule` cannot resolve. For non-`data` capsule types,
/// resolution requires `browser/index.html` inside the installed capsule dir —
/// the exact seam that dropped `creator` from the shell when its manifest moved
/// to `type: wasm` while its entrypoint stayed at the capsule root. Catalog
/// presence (the tests above) does NOT imply launchability, so pin it here.
#[test]
fn dkms_browser_capsules_stay_shell_launchable() {
    let data_dir = tempfile::tempdir().unwrap();
    install_all_dkms_capsules(data_dir.path());

    let launchable: std::collections::BTreeSet<String> =
        crate::api::browser_capsules::list_launchable_browser_capsules(data_dir.path())
            .into_iter()
            .map(|capsule| capsule.name)
            .collect();

    for name in ["creator", "elacity-player", "ddrm-viewer"] {
        assert!(
            launchable.contains(name),
            "{name} dropped from the shell launcher; launchable={launchable:?}"
        );
    }
}

/// Provisioning-registry pin: a capsule that exists in `capsules/` but has no entry in the
/// repo-root `components.json` is never installed into the data dir by
/// `scripts/setup-source-home.sh`, so it can pass every manifest/launchability test above
/// and still be absent from the Home desktop (that is exactly how `creator` went missing
/// after the ESP port). Every browser-facing dkms capsule must be registered for install.
#[test]
fn dkms_browser_capsules_are_registered_for_provisioning() {
    let raw = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../components.json"
    ))
    .expect("repo-root components.json must be readable");
    let manifest: serde_json::Value =
        serde_json::from_slice(&raw).expect("components.json must parse");
    let external = manifest["external"]
        .as_object()
        .expect("components.json must carry an external component map");

    for name in ["creator", "elacity-player", "ddrm-viewer"] {
        let entry = external.get(name).unwrap_or_else(|| {
            panic!("{name} missing from components.json external map — provisioning will never install it and the Home desktop will not show it")
        });
        assert_eq!(
            entry["install_path"].as_str(),
            Some(format!("capsules/{name}").as_str()),
            "{name} components.json entry must install into capsules/{name}"
        );
    }
}

/// Same install, exercised through the actual catalog read model
/// (`gateway_capsule_catalog::capsule_catalog_summary`, the function behind
/// `/api/capsules/catalog`) rather than the lower-level inventory list, so a
/// regression in catalog assembly (not just manifest parsing) is also caught.
#[test]
fn dkms_capsules_validate_against_the_esp_catalog_summary() {
    let data_dir = tempfile::tempdir().unwrap();
    install_all_dkms_capsules(data_dir.path());

    let catalog = capsule_catalog_summary(data_dir.path());
    let catalog_names: std::collections::BTreeSet<&str> = catalog
        .capsules
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();

    for name in DKMS_CAPSULES {
        assert!(
            catalog_names.contains(name),
            "{name} missing from the ESP capsule catalog summary; present={catalog_names:?}"
        );
    }
}
